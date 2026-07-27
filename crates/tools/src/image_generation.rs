use std::fs;
use std::path::{Path, PathBuf};

use agent_core::{
    Metadata, PermissionRequest, PermissionRisk, ToolArtifact, ToolInvocation, ToolOutcomeStatus,
    ToolResult, ToolRisk, ToolSpec,
};
use model_provider::{
    ImageGenerationRequest, OpenAiCompatibleImageConfig, OpenAiCompatibleImageProvider,
    MODEL_REQUEST_CANCELLED,
};

use super::{
    current_time_millis, parse_input, permission_request, required_input, resolve_workspace_path,
    ImageGenerationConfig, Tool, ToolError, ToolExecutionControl,
};

pub struct ImageGenerationTool {
    workspace_root: PathBuf,
    config: ImageGenerationConfig,
}

impl ImageGenerationTool {
    pub fn new(workspace_root: impl Into<PathBuf>, config: ImageGenerationConfig) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            config,
        }
    }
}

impl Tool for ImageGenerationTool {
    fn spec(&self) -> ToolSpec {
        let mut spec = ToolSpec::new(
            "image.generate",
            "image",
            format!(
                "Generate one raster image using the user-configured model `{}` and save it in the active workspace. The model and provider are controlled by Settings and cannot be overridden in tool input.",
                self.config.model
            ),
            ToolRisk::UsesNetwork,
            agent_core::ToolSource::BuiltIn,
            agent_core::ToolExposure::Auto,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "A detailed visual description of the image to generate."
                    },
                    "size": {
                        "type": "string",
                        "description": "Optional provider-supported size such as 1024x1024."
                    },
                    "output_path": {
                        "type": "string",
                        "description": "Optional workspace-relative output path. The file extension is normalized to the returned image format."
                    }
                },
                "required": ["prompt"],
                "additionalProperties": false
            })
            .to_string(),
        );
        spec.output_schema_json = Some(
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "mimeType": { "type": "string" },
                    "model": { "type": "string" }
                },
                "required": ["path", "mimeType", "model"]
            })
            .to_string(),
        );
        spec
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        let input = parse_input(&invocation.input_json);
        let output_path = input
            .get("output_path")
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| "generated-images/".to_string());
        Some(permission_request(
            &invocation.task_id,
            PermissionRisk::Network,
            "image.generate",
            "Send a visual prompt to the configured image provider and write the result in the workspace.",
            &output_path,
            [
                ("tool_call_id".to_string(), invocation.id.0.clone()),
                ("tool_name".to_string(), invocation.tool_name.clone()),
                ("model".to_string(), self.config.model.clone()),
            ]
            .into_iter()
            .collect(),
        ))
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        self.execute_with_control(invocation, &ToolExecutionControl::never_cancelled())
    }

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        if control.should_cancel() {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "Image generation cancelled before it started.",
                Metadata::new(),
            ));
        }
        let input = parse_input(&invocation.input_json);
        let prompt = required_input(&input, "prompt")?;
        let size = input
            .get("size")
            .filter(|value| !value.trim().is_empty())
            .cloned();
        let provider = OpenAiCompatibleImageProvider::new(OpenAiCompatibleImageConfig {
            base_url: self.config.base_url.clone(),
            api_key: self.config.api_key.clone(),
            model: self.config.model.clone(),
            timeout_seconds: self.config.timeout_seconds.max(1),
        });
        let response = match provider.generate_cancellable(
            ImageGenerationRequest {
                prompt,
                size: size.clone(),
                metadata: Metadata::new(),
            },
            || control.should_cancel(),
        ) {
            Ok(response) => response,
            Err(error) if error.message == MODEL_REQUEST_CANCELLED => {
                return Ok(ToolResult::text(
                    invocation.id,
                    ToolOutcomeStatus::Cancelled,
                    "Image generation cancelled.",
                    Metadata::new(),
                ));
            }
            Err(error) => return Err(ToolError::new(error.message)),
        };
        let image = response
            .images
            .into_iter()
            .next()
            .ok_or_else(|| ToolError::new("image provider returned no image"))?;
        if control.should_cancel() {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "Image generation cancelled.",
                Metadata::new(),
            ));
        }

        let requested_path = input.get("output_path").map(String::as_str);
        let (resolved_path, relative_path) =
            image_output_path(&self.workspace_root, requested_path, &image.mime_type)?;
        if let Some(parent) = resolved_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ToolError::new(format!("failed to create image output directory: {error}"))
            })?;
        }
        fs::write(&resolved_path, &image.bytes)
            .map_err(|error| ToolError::new(format!("failed to write generated image: {error}")))?;

        let mut metadata = Metadata::new();
        metadata.insert("artifact_path".to_string(), relative_path.clone());
        metadata.insert("model".to_string(), response.model.clone());
        metadata.insert("mime_type".to_string(), image.mime_type.clone());
        metadata.insert("bytes".to_string(), image.bytes.len().to_string());
        if let Some(size) = size {
            metadata.insert("size".to_string(), size);
        }
        let title = resolved_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Generated image")
            .to_string();
        let mut result = ToolResult::text(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            format!("Generated image: {relative_path}"),
            metadata,
        );
        result.artifacts.push(ToolArtifact {
            path: relative_path.clone(),
            mime_type: Some(image.mime_type.clone()),
            title: Some(title),
        });
        result.structured_output_json = Some(
            serde_json::json!({
                "path": relative_path,
                "mimeType": image.mime_type,
                "model": response.model,
                "revisedPrompt": image.revised_prompt
            })
            .to_string(),
        );
        Ok(result)
    }
}

pub(crate) fn image_output_path(
    workspace_root: &Path,
    requested_path: Option<&str>,
    mime_type: &str,
) -> Result<(PathBuf, String), ToolError> {
    let extension = match mime_type {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/png" => "png",
        _ => return Err(ToolError::new("unsupported generated image format")),
    };
    let timestamp = current_time_millis();
    let mut relative = requested_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!("generated-images/image-{timestamp}.{extension}"))
        });
    if relative.file_name().is_none() || requested_path.is_some_and(|value| value.ends_with('/')) {
        relative.push(format!("image-{timestamp}.{extension}"));
    } else {
        relative.set_extension(extension);
    }
    let relative_path = relative.to_string_lossy().to_string();
    let resolved = resolve_workspace_path(workspace_root, &relative_path)?;
    Ok((resolved, relative_path))
}
