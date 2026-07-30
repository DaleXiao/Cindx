use base64::Engine;
use std::collections::{HashSet, VecDeque};
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const PROVIDER_CATALOG_JSON: &str = include_str!("../providerCatalog.json");
const MAX_IMAGE_SOURCE_BYTES: usize = 24 * 1024 * 1024;
const PROVIDER_IMAGE_CACHE_BYTES: usize = 72 * 1024 * 1024;
const PROVIDER_IMAGE_CACHE_ENTRIES: usize = 10;
const GLOBAL_IMAGE_CACHE_BYTES: usize = 96 * 1024 * 1024;
static CATALOG_MODEL_MODALITIES: OnceLock<(HashSet<String>, HashSet<String>)> = OnceLock::new();
static GLOBAL_IMAGE_CACHE_BUDGET: OnceLock<Arc<GlobalImageCacheBudget>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
struct ImageFileIdentity {
    canonical_path: PathBuf,
    len: u64,
    #[cfg(unix)]
    unix: (u64, u64, i64, i64, i64, i64),
    #[cfg(not(unix))]
    modified_nanos: Option<u128>,
}

impl ImageFileIdentity {
    fn from_metadata(path: &Path, metadata: &Metadata) -> Option<Self> {
        let canonical_path = fs::canonicalize(path).ok()?;
        #[cfg(unix)]
        let unix = {
            use std::os::unix::fs::MetadataExt;
            (
                metadata.dev(),
                metadata.ino(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            )
        };
        #[cfg(not(unix))]
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_nanos());
        Some(Self {
            canonical_path,
            len: metadata.len(),
            #[cfg(unix)]
            unix,
            #[cfg(not(unix))]
            modified_nanos,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ImageCacheStats {
    pub(crate) hits: usize,
    pub(crate) misses: usize,
    pub(crate) reads: usize,
    pub(crate) encodes: usize,
    pub(crate) invalidations: usize,
    pub(crate) evictions: usize,
    pub(crate) bypasses: usize,
    pub(crate) retained_bytes: usize,
    pub(crate) entries: usize,
}

struct CachedImage {
    source_path: PathBuf,
    identity: ImageFileIdentity,
    data_url: Arc<str>,
}

#[derive(Default)]
struct ImageCacheState {
    entries: VecDeque<CachedImage>,
    stats: ImageCacheStats,
}

struct GlobalImageCacheBudget {
    limit: usize,
    retained: AtomicUsize,
}

impl GlobalImageCacheBudget {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            retained: AtomicUsize::new(0),
        }
    }

    fn try_reserve(&self, bytes: usize) -> bool {
        self.retained
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |retained| {
                retained
                    .checked_add(bytes)
                    .filter(|next| *next <= self.limit)
            })
            .is_ok()
    }

    fn release(&self, bytes: usize) {
        self.retained.fetch_sub(bytes, Ordering::AcqRel);
    }
}

pub(crate) struct ImageDataUrlCache {
    per_provider_limit: usize,
    per_provider_entries: usize,
    global_budget: Arc<GlobalImageCacheBudget>,
    state: Mutex<ImageCacheState>,
}

impl ImageDataUrlCache {
    pub(crate) fn new() -> Self {
        Self::with_limits(
            PROVIDER_IMAGE_CACHE_BYTES,
            PROVIDER_IMAGE_CACHE_ENTRIES,
            Arc::clone(
                GLOBAL_IMAGE_CACHE_BUDGET.get_or_init(|| {
                    Arc::new(GlobalImageCacheBudget::new(GLOBAL_IMAGE_CACHE_BYTES))
                }),
            ),
        )
    }

    fn with_limits(
        per_provider_limit: usize,
        per_provider_entries: usize,
        global_budget: Arc<GlobalImageCacheBudget>,
    ) -> Self {
        Self {
            per_provider_limit,
            per_provider_entries,
            global_budget,
            state: Mutex::new(ImageCacheState::default()),
        }
    }

    pub(crate) fn resolve(&self, value: &str) -> Option<Arc<str>> {
        let source_path = PathBuf::from(value.trim());
        let Some((mime_type, identity)) = validated_image_source(&source_path) else {
            self.invalidate(&source_path);
            return None;
        };
        if let Some(data_url) = self.lookup(&identity.canonical_path, &identity) {
            return Some(data_url);
        }
        self.update_stats(|stats| stats.misses += 1);
        let bytes = read_bounded_image(&source_path)?;
        self.update_stats(|stats| stats.reads += 1);
        let data_url = Arc::<str>::from(encoded_data_url(mime_type, &bytes));
        self.update_stats(|stats| stats.encodes += 1);
        let current_identity = fs::metadata(&source_path)
            .ok()
            .and_then(|metadata| ImageFileIdentity::from_metadata(&source_path, &metadata));
        if current_identity.as_ref() != Some(&identity) {
            self.invalidate(&source_path);
            self.update_stats(|stats| stats.bypasses += 1);
            return Some(data_url);
        }
        self.insert(source_path, identity, Arc::clone(&data_url));
        Some(data_url)
    }

    fn lookup(&self, canonical_path: &Path, identity: &ImageFileIdentity) -> Option<Arc<str>> {
        let mut state = self.state.lock().ok()?;
        let index = state
            .entries
            .iter()
            .position(|entry| entry.identity.canonical_path == canonical_path)?;
        let entry = state.entries.remove(index)?;
        if entry.identity == *identity {
            let data_url = Arc::clone(&entry.data_url);
            state.entries.push_back(entry);
            state.stats.hits += 1;
            return Some(data_url);
        }
        let bytes = entry.data_url.len();
        state.stats.retained_bytes = state.stats.retained_bytes.saturating_sub(bytes);
        state.stats.entries = state.entries.len();
        state.stats.invalidations += 1;
        self.global_budget.release(bytes);
        None
    }

    fn insert(&self, source_path: PathBuf, identity: ImageFileIdentity, data_url: Arc<str>) {
        let bytes = data_url.len();
        if bytes > self.per_provider_limit || self.per_provider_entries == 0 {
            self.update_stats(|stats| stats.bypasses += 1);
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if let Some(index) = state
            .entries
            .iter()
            .position(|entry| entry.identity.canonical_path == identity.canonical_path)
        {
            let existing = state.entries.remove(index).expect("cache index must exist");
            if existing.identity == identity {
                state.entries.push_back(existing);
                return;
            }
            let replaced_bytes = existing.data_url.len();
            state.stats.retained_bytes = state.stats.retained_bytes.saturating_sub(replaced_bytes);
            state.stats.entries = state.entries.len();
            state.stats.invalidations += 1;
            self.global_budget.release(replaced_bytes);
        }
        while state.entries.len() >= self.per_provider_entries
            || state.stats.retained_bytes.saturating_add(bytes) > self.per_provider_limit
        {
            let Some(evicted) = state.entries.pop_front() else {
                break;
            };
            let evicted_bytes = evicted.data_url.len();
            state.stats.retained_bytes = state.stats.retained_bytes.saturating_sub(evicted_bytes);
            state.stats.evictions += 1;
            state.stats.entries = state.entries.len();
            self.global_budget.release(evicted_bytes);
        }
        if !self.global_budget.try_reserve(bytes) {
            state.stats.bypasses += 1;
            return;
        }
        state.stats.retained_bytes += bytes;
        state.entries.push_back(CachedImage {
            source_path,
            identity,
            data_url,
        });
        state.stats.entries = state.entries.len();
    }

    fn invalidate(&self, source_path: &Path) {
        let canonical_path = fs::canonicalize(source_path).ok();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let mut removed_bytes = 0usize;
        let before = state.entries.len();
        state.entries.retain(|entry| {
            if entry.source_path == source_path
                || canonical_path.as_ref() == Some(&entry.identity.canonical_path)
            {
                removed_bytes = removed_bytes.saturating_add(entry.data_url.len());
                false
            } else {
                true
            }
        });
        if before != state.entries.len() {
            state.stats.invalidations += before - state.entries.len();
            state.stats.retained_bytes = state.stats.retained_bytes.saturating_sub(removed_bytes);
            state.stats.entries = state.entries.len();
            self.global_budget.release(removed_bytes);
        }
    }

    fn update_stats(&self, update: impl FnOnce(&mut ImageCacheStats)) {
        if let Ok(mut state) = self.state.lock() {
            update(&mut state.stats);
        }
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> ImageCacheStats {
        self.state
            .lock()
            .map(|state| state.stats)
            .unwrap_or_default()
    }
}

impl Drop for ImageDataUrlCache {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        self.global_budget.release(state.stats.retained_bytes);
        state.stats.retained_bytes = 0;
    }
}

pub fn model_supports_vision_content(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase().replace('_', "-");
    if let Some(supports_vision) = catalog_model_supports_vision(&model) {
        return supports_vision;
    }
    model.contains("vision")
        || model.contains("-vl")
        || model.contains("omni")
        || model.contains("pixtral")
        || model.contains("llava")
        || model.contains("glm-4v")
        || model.starts_with("gpt-4o")
        || model.starts_with("gpt-4.1")
        || model.starts_with("gpt-5")
        || model.starts_with("gemini")
        || model.starts_with("claude-3")
        || model.starts_with("claude-4")
}

fn catalog_model_supports_vision(model: &str) -> Option<bool> {
    let (chat, multimodal) = CATALOG_MODEL_MODALITIES.get_or_init(|| {
        let catalog: serde_json::Value = serde_json::from_str(PROVIDER_CATALOG_JSON)
            .expect("embedded provider catalog must be valid JSON");
        let mut chat = HashSet::new();
        let mut multimodal = HashSet::new();
        for provider in catalog["providers"].as_array().into_iter().flatten() {
            for candidate in provider["models"].as_array().into_iter().flatten() {
                let Some(id) = candidate["id"].as_str() else {
                    continue;
                };
                let id = id.trim().to_ascii_lowercase().replace('_', "-");
                let modalities = candidate["modalities"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>();
                if modalities.contains(&"chat") {
                    chat.insert(id.clone());
                }
                if modalities.contains(&"imageInput") {
                    multimodal.insert(id);
                }
            }
        }
        (chat, multimodal)
    });
    if multimodal.contains(model) {
        Some(true)
    } else if chat.contains(model) {
        Some(false)
    } else {
        None
    }
}

pub(crate) fn image_data_url(value: &str) -> Option<Arc<str>> {
    let path = PathBuf::from(value.trim());
    let (mime_type, _) = validated_image_source(&path)?;
    let bytes = read_bounded_image(&path)?;
    Some(Arc::<str>::from(encoded_data_url(mime_type, &bytes)))
}

fn validated_image_source(path: &Path) -> Option<(&'static str, ImageFileIdentity)> {
    if !path.is_absolute()
        || !path
            .components()
            .any(|component| component.as_os_str() == ".cindx")
    {
        return None;
    }
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_IMAGE_SOURCE_BYTES as u64 {
        return None;
    }
    let mime_type = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "avif" => "image/avif",
        "gif" => "image/gif",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => return None,
    };
    let identity = ImageFileIdentity::from_metadata(path, &metadata)?;
    Some((mime_type, identity))
}

fn read_bounded_image(path: &Path) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_IMAGE_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_IMAGE_SOURCE_BYTES).then_some(bytes)
}

fn encoded_data_url(mime_type: &str, bytes: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:{mime_type};base64,{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;

    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    fn fixture(name: &str, bytes: &[u8]) -> (PathBuf, PathBuf) {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "cindx-image-cache-{}-{sequence}-{name}",
            std::process::id()
        ));
        let path = root.join(".cindx").join(format!("{name}.png"));
        fs::create_dir_all(path.parent().expect("fixture should have a parent"))
            .expect("fixture directory should write");
        fs::write(&path, bytes).expect("fixture should write");
        (root, path)
    }

    fn cache(per_provider_limit: usize, global_limit: usize) -> ImageDataUrlCache {
        ImageDataUrlCache::with_limits(
            per_provider_limit,
            PROVIDER_IMAGE_CACHE_ENTRIES,
            Arc::new(GlobalImageCacheBudget::new(global_limit)),
        )
    }

    #[test]
    fn unchanged_image_hits_without_reread_or_reencode() {
        let (root, path) = fixture("hit", &[7; 1024]);
        let cache = cache(8 * 1024, 8 * 1024);

        let first = cache.resolve(path.to_str().unwrap()).unwrap();
        let second = cache.resolve(path.to_str().unwrap()).unwrap();
        let stats = cache.stats();
        let _ = fs::remove_dir_all(root);

        assert_eq!(first, second);
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.reads, 1);
        assert_eq!(stats.encodes, 1);
    }

    #[test]
    fn modified_and_deleted_images_invalidate_immediately() {
        let (root, path) = fixture("invalidate", &[1; 1024]);
        let cache = cache(8 * 1024, 8 * 1024);
        let first = cache.resolve(path.to_str().unwrap()).unwrap();

        fs::write(&path, [2; 2048]).expect("fixture should update");
        let second = cache.resolve(path.to_str().unwrap()).unwrap();
        fs::remove_file(&path).expect("fixture should delete");
        assert!(cache.resolve(path.to_str().unwrap()).is_none());
        let stats = cache.stats();
        let _ = fs::remove_dir_all(root);

        assert_ne!(first, second);
        assert_eq!(stats.reads, 2);
        assert_eq!(stats.encodes, 2);
        assert_eq!(stats.invalidations, 2);
        assert_eq!(stats.retained_bytes, 0);
    }

    #[test]
    fn provider_limit_evicts_lru_entries() {
        let (first_root, first_path) = fixture("evict-a", &[1; 1024]);
        let (second_root, second_path) = fixture("evict-b", &[2; 1024]);
        let cache = cache(1_500, 8 * 1024);

        cache.resolve(first_path.to_str().unwrap()).unwrap();
        cache.resolve(second_path.to_str().unwrap()).unwrap();
        cache.resolve(first_path.to_str().unwrap()).unwrap();
        let stats = cache.stats();
        let _ = fs::remove_dir_all(first_root);
        let _ = fs::remove_dir_all(second_root);

        assert_eq!(stats.reads, 3);
        assert_eq!(stats.encodes, 3);
        assert_eq!(stats.evictions, 2);
        assert!(stats.retained_bytes <= 1_500);
    }

    #[test]
    fn exhausted_global_budget_bypasses_without_failing() {
        let (root, path) = fixture("bypass", &[3; 1024]);
        let cache = cache(8 * 1024, 0);

        let first = cache.resolve(path.to_str().unwrap()).unwrap();
        let second = cache.resolve(path.to_str().unwrap()).unwrap();
        let stats = cache.stats();
        let _ = fs::remove_dir_all(root);

        assert_eq!(first, second);
        assert_eq!(stats.reads, 2);
        assert_eq!(stats.encodes, 2);
        assert_eq!(stats.bypasses, 2);
        assert_eq!(stats.retained_bytes, 0);
    }

    #[test]
    fn concurrent_provider_caches_share_global_retained_budget() {
        let (first_root, first_path) = fixture("global-a", &[4; 1024]);
        let (second_root, second_path) = fixture("global-b", &[5; 1024]);
        let global = Arc::new(GlobalImageCacheBudget::new(1_500));
        let first = Arc::new(ImageDataUrlCache::with_limits(
            8 * 1024,
            PROVIDER_IMAGE_CACHE_ENTRIES,
            Arc::clone(&global),
        ));
        let second = Arc::new(ImageDataUrlCache::with_limits(
            8 * 1024,
            PROVIDER_IMAGE_CACHE_ENTRIES,
            Arc::clone(&global),
        ));
        let barrier = Arc::new(Barrier::new(3));

        std::thread::scope(|scope| {
            for (cache, path) in [
                (Arc::clone(&first), first_path.as_path()),
                (Arc::clone(&second), second_path.as_path()),
            ] {
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    cache.resolve(path.to_str().unwrap()).unwrap();
                });
            }
            barrier.wait();
        });
        let first_stats = first.stats();
        let second_stats = second.stats();
        let _ = fs::remove_dir_all(first_root);
        let _ = fs::remove_dir_all(second_root);

        assert_eq!(first_stats.bypasses + second_stats.bypasses, 1);
        assert_eq!(first_stats.entries + second_stats.entries, 1);
        assert!(global.retained.load(Ordering::Acquire) <= 1_500);
    }

    #[test]
    fn concurrent_same_cache_resolves_retain_one_entry() {
        let (root, path) = fixture("same-source", &[6; 1024]);
        let global = Arc::new(GlobalImageCacheBudget::new(8 * 1024));
        let cache = Arc::new(ImageDataUrlCache::with_limits(
            8 * 1024,
            PROVIDER_IMAGE_CACHE_ENTRIES,
            Arc::clone(&global),
        ));
        let barrier = Arc::new(Barrier::new(9));
        let values = std::thread::scope(|scope| {
            let handles = (0..8)
                .map(|_| {
                    let cache = Arc::clone(&cache);
                    let barrier = Arc::clone(&barrier);
                    let path = path.as_path();
                    scope.spawn(move || {
                        barrier.wait();
                        cache.resolve(path.to_str().unwrap()).unwrap()
                    })
                })
                .collect::<Vec<_>>();
            barrier.wait();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        let cached = cache.resolve(path.to_str().unwrap()).unwrap();
        let stats = cache.stats();
        let _ = fs::remove_dir_all(root);

        assert_eq!(stats.entries, 1);
        assert_eq!(stats.retained_bytes, cached.len());
        assert_eq!(global.retained.load(Ordering::Acquire), cached.len());
        assert!(values.iter().any(|value| Arc::ptr_eq(value, &cached)));
    }

    #[test]
    fn raw_path_aliases_share_one_canonical_entry() {
        let (root, path) = fixture("alias", &[8; 1024]);
        let alias_directory = path.parent().unwrap().join("alias-directory");
        fs::create_dir(&alias_directory).expect("alias directory should write");
        let alias = alias_directory.join("..").join(path.file_name().unwrap());
        let cache = cache(8 * 1024, 8 * 1024);

        let first = cache.resolve(path.to_str().unwrap()).unwrap();
        let second = cache.resolve(alias.to_str().unwrap()).unwrap();
        let stats = cache.stats();
        let _ = fs::remove_dir_all(root);

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.reads, 1);
        assert_eq!(stats.encodes, 1);
    }

    #[test]
    fn provider_entry_limit_evicts_lru_entries() {
        let cache = ImageDataUrlCache::with_limits(
            64 * 1024,
            PROVIDER_IMAGE_CACHE_ENTRIES,
            Arc::new(GlobalImageCacheBudget::new(64 * 1024)),
        );
        let fixtures = (0..=PROVIDER_IMAGE_CACHE_ENTRIES)
            .map(|index| fixture(&format!("entry-{index}"), &[index as u8; 32]))
            .collect::<Vec<_>>();

        for (_, path) in &fixtures {
            cache.resolve(path.to_str().unwrap()).unwrap();
        }
        cache
            .resolve(fixtures[0].1.to_str().unwrap())
            .expect("oldest entry should reread after eviction");
        let stats = cache.stats();
        for (root, _) in fixtures {
            let _ = fs::remove_dir_all(root);
        }

        assert_eq!(PROVIDER_IMAGE_CACHE_ENTRIES, 10);
        assert_eq!(stats.entries, PROVIDER_IMAGE_CACHE_ENTRIES);
        assert_eq!(stats.reads, PROVIDER_IMAGE_CACHE_ENTRIES + 2);
        assert_eq!(stats.encodes, PROVIDER_IMAGE_CACHE_ENTRIES + 2);
        assert_eq!(stats.evictions, 2);
    }
}
