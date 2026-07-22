export const CINDX_MAX_MINOR = 10;
export const CINDX_MAX_PATCH = 100;

const MINOR_RADIX = CINDX_MAX_MINOR + 1;
const PATCH_RADIX = CINDX_MAX_PATCH + 1;

export function parseCindxVersion(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(String(version).trim());
  if (!match) throw new Error(`Unsupported Cindx version: ${version}`);
  const parsed = {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3])
  };
  if (parsed.minor > CINDX_MAX_MINOR || parsed.patch > CINDX_MAX_PATCH) {
    throw new Error(
      `Cindx version is outside the supported range: ${version} ` +
        `(minor 0-${CINDX_MAX_MINOR}, patch 0-${CINDX_MAX_PATCH})`
    );
  }
  return parsed;
}

export function cindxVersionOrdinal(version) {
  const { major, minor, patch } = parseCindxVersion(version);
  return (major * MINOR_RADIX + minor) * PATCH_RADIX + patch;
}

export function cindxVersionFromOrdinal(ordinal) {
  if (!Number.isSafeInteger(ordinal) || ordinal < 0) {
    throw new Error(`Cindx build ordinal must be a non-negative integer: ${ordinal}`);
  }
  const patch = ordinal % PATCH_RADIX;
  const majorMinor = Math.floor(ordinal / PATCH_RADIX);
  const minor = majorMinor % MINOR_RADIX;
  const major = Math.floor(majorMinor / MINOR_RADIX);
  return `${major}.${minor}.${patch}`;
}

export function nextCindxVersion(currentVersion, minimumOrdinal) {
  const currentOrdinal = cindxVersionOrdinal(currentVersion);
  if (minimumOrdinal === undefined || minimumOrdinal === null) {
    return cindxVersionFromOrdinal(currentOrdinal + 1);
  }
  const requested = Number(minimumOrdinal);
  if (!Number.isSafeInteger(requested) || requested < 1) {
    throw new Error(`Cindx build ordinal must be a positive integer: ${minimumOrdinal}`);
  }
  return cindxVersionFromOrdinal(Math.max(currentOrdinal + 1, requested));
}
