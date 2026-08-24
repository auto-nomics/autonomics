use async_trait::async_trait;

use crate::ContainerRuntimeError;
use crate::PodmanRuntime;
use crate::process::{PodmanCommandOutput, execute_podman};
use crate::types::PullPolicy;

use super::parser::{parse_image_inspect, parse_image_records};
use super::types::{
    EnsureImageResult, ImageInspect, ImageListOptions, ImageManager, ImagePullResult, ImageRecord,
    ImageRemoveOptions, ImageRemoveResult,
};

const DEFAULT_IMAGE_TIMEOUT_SECS: u64 = crate::DEFAULT_TIMEOUT_SECS;
const MAX_IMAGE_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

#[async_trait]
impl ImageManager for PodmanRuntime {
    async fn image_exists(&self, image: &str) -> Result<bool, ContainerRuntimeError> {
        let output = self.podman_image_command(&image_exists_args(image)).await?;
        if output.success {
            Ok(true)
        } else if output.exit_code == 1 {
            Ok(false)
        } else {
            Err(command_error("image exists", output))
        }
    }

    async fn image_pull(&self, image: &str) -> Result<ImagePullResult, ContainerRuntimeError> {
        let output = self
            .podman_image_command(&image_pull_args(image, PullPolicy::Always))
            .await?;
        if !output.success {
            return Err(command_error("image pull", output));
        }
        Ok(ImagePullResult {
            image: image.to_string(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    async fn image_inspect(&self, image: &str) -> Result<ImageInspect, ContainerRuntimeError> {
        let output = self
            .podman_image_command(&image_inspect_args(image))
            .await?;
        if !output.success {
            return Err(command_error("image inspect", output));
        }
        parse_image_inspect(&output.stdout)
    }

    async fn image_list(
        &self,
        options: ImageListOptions,
    ) -> Result<Vec<ImageRecord>, ContainerRuntimeError> {
        options.validate()?;
        let output = self
            .podman_image_command(&image_list_args(&options))
            .await?;
        if !output.success {
            return Err(command_error("image list", output));
        }
        parse_image_records(&output.stdout)
    }

    async fn image_remove(
        &self,
        image: &str,
        options: ImageRemoveOptions,
    ) -> Result<ImageRemoveResult, ContainerRuntimeError> {
        let output = self
            .podman_image_command(&image_remove_args(image, options))
            .await?;
        if !output.success {
            return Err(command_error("image remove", output));
        }
        Ok(ImageRemoveResult {
            image: image.to_string(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    async fn ensure_image(
        &self,
        image: &str,
        policy: PullPolicy,
    ) -> Result<EnsureImageResult, ContainerRuntimeError> {
        validate_image_ref(image)?;
        let pull = match policy {
            PullPolicy::Never => None,
            PullPolicy::Missing if self.image_exists(image).await? => None,
            PullPolicy::Missing | PullPolicy::Always | PullPolicy::Newer => {
                let output = self
                    .podman_image_command(&image_pull_args(image, policy))
                    .await?;
                if !output.success {
                    return Err(command_error("image pull", output));
                }
                Some(ImagePullResult {
                    image: image.to_string(),
                    stdout: output.stdout,
                    stderr: output.stderr,
                })
            }
        };

        let inspected = self.image_inspect(image).await?;
        Ok(EnsureImageResult {
            pulled: pull.is_some(),
            pull,
            image: inspected,
        })
    }
}

impl PodmanRuntime {
    async fn podman_image_command(
        &self,
        args: &[String],
    ) -> Result<PodmanCommandOutput, ContainerRuntimeError> {
        if let Some(error) = validate_image_args(args) {
            return Err(error);
        }
        let mut full_args = vec!["image".to_string()];
        full_args.extend_from_slice(args);
        execute_podman(
            &self.binary,
            &full_args,
            DEFAULT_IMAGE_TIMEOUT_SECS,
            MAX_IMAGE_OUTPUT_BYTES,
        )
        .await
    }
}

fn validate_image_ref(image: &str) -> Result<(), ContainerRuntimeError> {
    if image.trim().is_empty() || image.contains('\0') || image.chars().any(char::is_whitespace) {
        return Err(ContainerRuntimeError::Invalid(format!(
            "invalid image reference `{image}`"
        )));
    }
    Ok(())
}

fn validate_image_args(args: &[String]) -> Option<ContainerRuntimeError> {
    args.iter().find_map(|arg| {
        arg.contains('\0').then(|| {
            ContainerRuntimeError::Invalid(
                "image command arguments cannot contain NUL bytes".into(),
            )
        })
    })
}

pub(super) fn image_exists_args(image: &str) -> Vec<String> {
    vec!["exists".into(), image.to_string()]
}

pub(super) fn image_inspect_args(image: &str) -> Vec<String> {
    vec![
        "inspect".into(),
        "--format={{json .}}".into(),
        image.to_string(),
    ]
}

pub(super) fn image_list_args(options: &ImageListOptions) -> Vec<String> {
    let mut args = vec!["ls".into(), "--format={{json .}}".into()];
    for filter in &options.filters {
        args.push("--filter".into());
        args.push(filter.clone());
    }
    args
}

pub(super) fn image_pull_args(image: &str, policy: PullPolicy) -> Vec<String> {
    vec![
        "pull".into(),
        format!("--policy={}", policy.as_cli_value()),
        image.to_string(),
    ]
}

pub(super) fn image_remove_args(image: &str, options: ImageRemoveOptions) -> Vec<String> {
    let mut args = vec!["rm".into()];
    if options.force {
        args.push("--force".into());
    }
    if options.ignore_missing {
        args.push("--ignore".into());
    }
    args.push(image.to_string());
    args
}

fn command_error(command: &'static str, output: PodmanCommandOutput) -> ContainerRuntimeError {
    ContainerRuntimeError::Command {
        command,
        exit_code: output.exit_code,
        stderr: output.stderr,
    }
}

#[cfg(test)]
mod tests {
    use crate::{ImageManager, PodmanRuntime};

    #[tokio::test]
    async fn test_list_images() {
        let runtime = PodmanRuntime::new("podman");
        let images = runtime
            .image_list(crate::ImageListOptions { filters: vec![] })
            .await
            .unwrap();
        dbg!(images);
    }
}
