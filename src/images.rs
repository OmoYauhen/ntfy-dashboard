//! Disk cache for dish photos, so day switching never waits for the NTFY image server.
//! Images are immutable (the id is a content UUID), so cached files never need refreshing.

use std::path::PathBuf;

use anyhow::Result;
use bytes::Bytes;
use futures::StreamExt;
use serde_json::Value;

use crate::orion::Client;

/// Resolution used by the tiles; the details card asks for a sharper one on demand.
pub const TILE_RESOLUTION: &str = "MEDIUM";
const PREFETCH_CONCURRENCY: usize = 6;

pub struct ImageCache {
    dir: PathBuf,
}

impl ImageCache {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn path(&self, id: &str, resolution: &str) -> Option<PathBuf> {
        // Ids come from the URL: only allow plain file names.
        let safe = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && !s.starts_with('.');
        (safe(id) && safe(resolution)).then(|| self.dir.join(format!("{resolution}_{id}")))
    }

    /// Returns the image, from disk if cached, otherwise downloaded and stored.
    pub async fn get(&self, client: &Client, id: &str, resolution: &str) -> Result<(String, Bytes)> {
        let path = self.path(id, resolution).ok_or_else(|| anyhow::anyhow!("invalid image id"))?;
        if let Ok(bytes) = tokio::fs::read(&path).await {
            return Ok((content_type(id).to_string(), bytes.into()));
        }
        let (content_type, bytes) = client.image(id, resolution).await?;
        // Unique temp name: two requests for the same image may race.
        let tmp = self.dir.join(format!(".{}.part", uuid::Uuid::new_v4()));
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok((content_type, bytes))
    }

    /// Downloads every not-yet-cached tile image of the given products. Returns how many were fetched.
    pub async fn prefetch(&self, client: &Client, image_ids: Vec<String>) -> usize {
        let missing: Vec<String> = image_ids
            .into_iter()
            .filter(|id| self.path(id, TILE_RESOLUTION).is_some_and(|p| !p.exists()))
            .collect();
        futures::stream::iter(missing)
            .map(|id| async move { self.get(client, &id, TILE_RESOLUTION).await.is_ok() })
            .buffer_unordered(PREFETCH_CONCURRENCY)
            .filter(|ok| std::future::ready(*ok))
            .count()
            .await
    }
}

fn content_type(id: &str) -> &'static str {
    match id.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/jpeg",
    }
}

/// Main image ids of all products in an API response's `includes.simple_products`.
pub fn product_image_ids(data: &Value) -> Vec<String> {
    let mut ids: Vec<String> = data["includes"]["simple_products"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let images = p["images"].as_array()?;
            let main = images.iter().find(|i| i["type"] == "MAIN").or(images.first())?;
            main["id"].as_str().map(str::to_string)
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal() {
        let cache = ImageCache { dir: PathBuf::from("/tmp/x") };
        assert!(cache.path("../../etc/passwd", "MEDIUM").is_none());
        assert!(cache.path("a/b.jpg", "MEDIUM").is_none());
        assert!(cache.path("9e553ae0-1911.jpg", "MEDIUM/..").is_none());
        assert_eq!(cache.path("9e553ae0-1911.jpg", "MEDIUM").unwrap(), PathBuf::from("/tmp/x/MEDIUM_9e553ae0-1911.jpg"));
    }

    #[test]
    fn collects_main_images() {
        let data = serde_json::json!({"includes": {"simple_products": [
            {"images": [{"id": "b.jpg", "type": "EXTRA"}, {"id": "a.jpg", "type": "MAIN"}]},
            {"images": [{"id": "a.jpg", "type": "MAIN"}]},
            {"images": []}
        ]}});
        assert_eq!(product_image_ids(&data), ["a.jpg"]);
    }
}
