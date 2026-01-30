//! Client for mvirt-zfs daemon.

use anyhow::{Context, Result};
use tonic::transport::Channel;
use tracing::debug;

use crate::proto::zfs::{
    zfs_service_client::ZfsServiceClient, CloneFromTemplateRequest, CreateVolumeRequest,
    DeleteTemplateRequest, DeleteVolumeRequest, GetImportJobRequest, GetVolumeRequest,
    ImportTemplateRequest, ListTemplatesRequest, ListVolumesRequest,
};

pub use crate::proto::zfs::{ImportJob, Template, Volume};

/// Client for interacting with mvirt-zfs.
#[derive(Clone)]
pub struct ZfsClient {
    client: ZfsServiceClient<Channel>,
}

impl ZfsClient {
    pub async fn connect(endpoint: &str) -> Result<Self> {
        let client = ZfsServiceClient::connect(endpoint.to_string())
            .await
            .context("Failed to connect to mvirt-zfs")?;
        Ok(Self { client })
    }

    /// List all volumes.
    pub async fn list_volumes(&mut self) -> Result<Vec<Volume>> {
        debug!("Listing volumes from mvirt-zfs");
        let resp = self
            .client
            .list_volumes(ListVolumesRequest {})
            .await
            .context("Failed to list volumes")?;
        Ok(resp.into_inner().volumes)
    }

    /// Get volume by name.
    pub async fn get_volume(&mut self, name: &str) -> Result<Option<Volume>> {
        debug!("Getting volume {} from mvirt-zfs", name);
        match self
            .client
            .get_volume(GetVolumeRequest {
                name: name.to_string(),
            })
            .await
        {
            Ok(resp) => Ok(Some(resp.into_inner())),
            Err(status) if status.code() == tonic::Code::NotFound => Ok(None),
            Err(e) => Err(e).context("Failed to get volume"),
        }
    }

    /// Create a volume.
    pub async fn create_volume(&mut self, name: &str, size_bytes: u64) -> Result<Volume> {
        debug!("Creating volume {} ({}B) in mvirt-zfs", name, size_bytes);
        let resp = self
            .client
            .create_volume(CreateVolumeRequest {
                name: name.to_string(),
                size_bytes,
                volblocksize: None,
            })
            .await
            .context("Failed to create volume")?;
        Ok(resp.into_inner())
    }

    /// Clone a volume from a template.
    pub async fn clone_from_template(
        &mut self,
        template_name: &str,
        new_volume_name: &str,
        size_bytes: Option<u64>,
    ) -> Result<Volume> {
        debug!(
            "Cloning volume {} from template {} in mvirt-zfs",
            new_volume_name, template_name
        );
        let resp = self
            .client
            .clone_from_template(CloneFromTemplateRequest {
                template_name: template_name.to_string(),
                new_volume_name: new_volume_name.to_string(),
                size_bytes,
            })
            .await
            .context("Failed to clone from template")?;
        Ok(resp.into_inner())
    }

    /// Delete a volume.
    pub async fn delete_volume(&mut self, name: &str) -> Result<()> {
        debug!("Deleting volume {} in mvirt-zfs", name);
        self.client
            .delete_volume(DeleteVolumeRequest {
                name: name.to_string(),
            })
            .await
            .context("Failed to delete volume")?;
        Ok(())
    }

    /// Import a template from URL or path.
    pub async fn import_template(
        &mut self,
        name: &str,
        source: &str,
        size_bytes: Option<u64>,
    ) -> Result<ImportJob> {
        debug!("Importing template {} from {} in mvirt-zfs", name, source);
        let resp = self
            .client
            .import_template(ImportTemplateRequest {
                name: name.to_string(),
                source: source.to_string(),
                size_bytes,
            })
            .await
            .context("Failed to import template")?;
        Ok(resp.into_inner())
    }

    /// Get import job status by ID.
    pub async fn get_import_job(&mut self, id: &str) -> Result<ImportJob> {
        let resp = self
            .client
            .get_import_job(GetImportJobRequest { id: id.to_string() })
            .await
            .context("Failed to get import job")?;
        Ok(resp.into_inner())
    }

    /// List all templates.
    pub async fn list_templates(&mut self) -> Result<Vec<Template>> {
        debug!("Listing templates from mvirt-zfs");
        let resp = self
            .client
            .list_templates(ListTemplatesRequest {})
            .await
            .context("Failed to list templates")?;
        Ok(resp.into_inner().templates)
    }

    /// Delete a template.
    pub async fn delete_template(&mut self, name: &str) -> Result<()> {
        debug!("Deleting template {} in mvirt-zfs", name);
        self.client
            .delete_template(DeleteTemplateRequest {
                name: name.to_string(),
            })
            .await
            .context("Failed to delete template")?;
        Ok(())
    }
}
