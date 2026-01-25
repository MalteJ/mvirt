//! OCI Registry Client - Pulls images from container registries.
//!
//! Based on FeOS image-service/worker.rs pattern.

use super::{PulledImageData, PulledLayer};
use crate::error::ImageError;
use log::{info, warn};
use oci_distribution::{Client, Reference, client::ClientConfig, manifest, secrets::RegistryAuth};

/// Pull an OCI image from a registry.
pub async fn pull_oci_image(image_ref: &str) -> Result<PulledImageData, ImageError> {
    info!("ImagePuller: Fetching image: {}", image_ref);

    let reference = Reference::try_from(image_ref.to_string())
        .map_err(|e| ImageError::InvalidReference(e.to_string()))?;

    let accepted_media_types = [
        manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE,
        manifest::IMAGE_DOCKER_LAYER_GZIP_MEDIA_TYPE,
    ];

    let config = ClientConfig {
        ..Default::default()
    };
    let client = Client::new(config);
    let auth = &RegistryAuth::Anonymous;

    info!("ImagePuller: Pulling manifest and config for {}", image_ref);
    let (manifest, _, _) = client
        .pull_manifest_and_config(&reference, auth)
        .await
        .map_err(|e| ImageError::Registry(e.to_string()))?;

    let mut config_data = Vec::new();
    client
        .pull_blob(&reference, &manifest.config, &mut config_data)
        .await
        .map_err(|e| ImageError::Registry(e.to_string()))?;
    info!(
        "ImagePuller: Pulled config blob ({} bytes)",
        config_data.len()
    );

    let mut layers = Vec::new();
    for layer in manifest.layers {
        if !accepted_media_types.contains(&layer.media_type.as_str()) {
            warn!(
                "ImagePuller: Skipping layer with unsupported media type: {}",
                layer.media_type
            );
            continue;
        }

        info!(
            "ImagePuller: Pulling layer {} ({})",
            layer.digest, layer.media_type
        );

        let mut layer_data = Vec::new();
        client
            .pull_blob(&reference, &layer, &mut layer_data)
            .await
            .map_err(|e| ImageError::Registry(e.to_string()))?;
        info!(
            "ImagePuller: Pulled layer blob ({} bytes)",
            layer_data.len()
        );

        layers.push(PulledLayer {
            media_type: layer.media_type.clone(),
            data: layer_data,
        });
    }

    if layers.is_empty() {
        return Err(ImageError::LayerExtraction(
            "No compatible layers found".to_string(),
        ));
    }

    Ok(PulledImageData {
        config: config_data,
        layers,
    })
}
