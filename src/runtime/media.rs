//! Task-owned attachment snapshots. Never rewrite the caller's instruction.
use crate::applications::wecom::nickname_native::Attachment;
use crate::kernel::instruction::Message;
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const IMAGE_LIMIT: u64 = 20 * 1024 * 1024;
const FILE_LIMIT: u64 = 100 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 200 * 1024 * 1024;

pub(super) struct PreparedMessages {
    root: Option<PathBuf>,
    attachments: Vec<Option<Attachment>>,
    // Once paste might have started, unknown outcomes retain private resources
    // for reconciliation: an application may still be reading a file URL.
    retain: AtomicBool,
}

impl PreparedMessages {
    pub(super) fn prepare(messages: &[Message]) -> Result<Self> {
        let mut prepared = Self {
            root: None,
            attachments: Vec::with_capacity(messages.len()),
            retain: AtomicBool::new(false),
        };
        let mut total = 0u64;
        for message in messages {
            let (path, filename, mime_type, is_image) = match message {
                Message::Text { .. } => {
                    prepared.attachments.push(None);
                    continue;
                }
                Message::Image { path, filename, mime_type } => (path, filename, mime_type, true),
                Message::File { path, filename, mime_type } => (path, filename, mime_type, false),
            };
            let original = Path::new(path);
            let before = std::fs::symlink_metadata(original).context("Attachment unavailable")?;
            ensure!(before.is_file(), "Attachment must be a regular non-symlink file");
            let limit = if is_image { IMAGE_LIMIT } else { FILE_LIMIT };
            ensure!(before.len() > 0 && before.len() <= limit, "Attachment size outside limit");
            total = total.checked_add(before.len()).context("Attachment size overflow")?;
            ensure!(total <= TOTAL_LIMIT, "Total attachment size outside limit");
            let mut source = File::open(original).context("Attachment unavailable")?;
            let opened = source.metadata()?;
            ensure!(opened.is_file() && same_file(&before, &opened), "Attachment changed before snapshot");
            let root = match &prepared.root {
                Some(root) => root.clone(),
                None => {
                    let root = std::env::temp_dir().join(format!("desk-agent-media-{}", uuid::Uuid::new_v4()));
                    private_directory(&root)?;
                    prepared.root = Some(root.clone());
                    root
                }
            };
            let directory = root.join(prepared.attachments.len().to_string());
            private_directory(&directory)?;
            let staged_path = directory.join(filename);
            let mut destination = private_file(&staged_path)?;
            let (size_bytes, sha256) = copy_and_hash(&mut source, &mut destination, limit)?;
            ensure!(size_bytes == opened.len() && same_file(&opened, &source.metadata()?), "Attachment changed while snapshotting");
            destination.sync_all()?;
            drop(destination);
            readonly_private(&staged_path)?;
            if is_image {
                validate_image(&staged_path, mime_type)?;
            }
            prepared.attachments.push(Some(Attachment {
                path: staged_path,
                filename: filename.clone(),
                mime_type: mime_type.clone(),
                is_image,
                sha256,
                size_bytes,
            }));
        }
        Ok(prepared)
    }

    pub(super) fn get(&self, index: usize) -> Option<&Attachment> {
        self.attachments.get(index).and_then(Option::as_ref)
    }

    pub(super) fn validate(&self, index: usize) -> Result<()> {
        if let Some(attachment) = self.get(index) {
            let metadata = std::fs::symlink_metadata(&attachment.path)?;
            ensure!(metadata.is_file() && metadata.len() == attachment.size_bytes, "Attachment snapshot changed");
            let mut file = File::open(&attachment.path)?;
            ensure!(same_file(&metadata, &file.metadata()?), "Attachment snapshot replaced");
            let (size, digest) = hash(&mut file, attachment.size_bytes)?;
            ensure!(size == attachment.size_bytes && digest == attachment.sha256, "Attachment snapshot changed");
        }
        Ok(())
    }

    pub(super) fn validate_all(&self) -> Result<()> {
        for index in 0..self.attachments.len() { self.validate(index)?; }
        Ok(())
    }

    pub(super) fn binding(&self, index: usize) -> serde_json::Value {
        match self.get(index) {
            Some(a) => serde_json::json!({"sha256": a.sha256, "size_bytes": a.size_bytes, "mime_type": a.mime_type, "filename": a.filename, "is_image": a.is_image}),
            None => serde_json::Value::Null,
        }
    }

    pub(super) fn mark_paste_started(&self, index: usize) {
        if self.get(index).is_some() { self.retain.store(true, Ordering::Relaxed); }
    }

    pub(super) fn release_completed(&self) { self.retain.store(false, Ordering::Relaxed); }
}

impl Drop for PreparedMessages {
    fn drop(&mut self) {
        if !self.retain.load(Ordering::Relaxed) {
            if let Some(root) = &self.root {
                if std::fs::remove_dir_all(root).is_err() {
                    tracing::warn!("Private attachment snapshot cleanup failed");
                }
            }
        }
    }
}

#[cfg(unix)]
fn private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)?;
    Ok(())
}
#[cfg(not(unix))]
fn private_directory(_: &Path) -> Result<()> { anyhow::bail!("Private attachment storage requires a supported platform") }

#[cfg(unix)]
fn private_file(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?)
}
#[cfg(not(unix))]
fn private_file(_: &Path) -> Result<File> { anyhow::bail!("Private attachment storage requires a supported platform") }

#[cfg(unix)]
fn readonly_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400))?;
    Ok(())
}
#[cfg(not(unix))]
fn readonly_private(_: &Path) -> Result<()> { anyhow::bail!("Private attachment storage requires a supported platform") }

fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    #[cfg(unix)] {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev() && a.ino() == b.ino() && a.len() == b.len()
            && a.mtime() == b.mtime() && a.mtime_nsec() == b.mtime_nsec()
    }
    #[cfg(not(unix))] { a.len() == b.len() && a.modified().ok() == b.modified().ok() }
}

fn copy_and_hash(source: &mut File, destination: &mut File, limit: u64) -> Result<(u64, String)> {
    let mut digest = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 { break; }
        count += read as u64;
        ensure!(count <= limit, "Attachment exceeds size limit");
        digest.update(&buffer[..read]);
        destination.write_all(&buffer[..read])?;
    }
    Ok((count, format!("{:x}", digest.finalize())))
}

fn hash(source: &mut File, limit: u64) -> Result<(u64, String)> {
    let mut digest = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 { break; }
        count += read as u64;
        ensure!(count <= limit, "Attachment snapshot exceeds size limit");
        digest.update(&buffer[..read]);
    }
    Ok((count, format!("{:x}", digest.finalize())))
}

fn validate_image(path: &Path, mime_type: &str) -> Result<()> {
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let expected = match mime_type {
        "image/png" => image::ImageFormat::Png,
        "image/jpeg" => image::ImageFormat::Jpeg,
        _ => anyhow::bail!("Unsupported image MIME type"),
    };
    ensure!(reader.format() == Some(expected), "Image MIME type does not match original bytes");
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().context("Image is malformed or exceeds decode limits")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("desk-agent-media-test-{}", uuid::Uuid::new_v4()));
            private_directory(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str, bytes: &[u8]) -> Message {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            Message::File { path: path.to_str().unwrap().into(), filename: name.into(), mime_type: "application/octet-stream".into() }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn original_replacement_does_not_change_task_bytes_and_completed_resources_are_removed() {
        let fixture = Fixture::new();
        let message = fixture.file("original.bin", b"authorized original bytes");
        let prepared = PreparedMessages::prepare(&[message]).unwrap();
        let attachment = prepared.get(0).unwrap();
        let staged = attachment.path.clone();
        assert_eq!(attachment.filename, "original.bin");
        std::fs::write(fixture.0.join("original.bin"), b"replaced source").unwrap();
        prepared.validate(0).unwrap();
        assert_eq!(std::fs::read(&staged).unwrap(), b"authorized original bytes");
        #[cfg(unix)] {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(std::fs::metadata(&staged).unwrap().mode() & 0o777, 0o400);
            assert_eq!(std::fs::metadata(staged.parent().unwrap()).unwrap().mode() & 0o777, 0o700);
        }
        drop(prepared);
        assert!(!staged.exists());
    }

    #[test]
    fn declared_image_type_must_match_decodable_original_bytes() {
        let fixture = Fixture::new();
        let path = fixture.0.join("original.png");
        image::RgbImage::from_pixel(2, 3, image::Rgb([10, 20, 30])).save(&path).unwrap();
        let mut message = Message::Image { path: path.to_str().unwrap().into(), filename: "original.png".into(), mime_type: "image/jpeg".into() };
        assert!(PreparedMessages::prepare(&[message.clone()]).is_err());
        if let Message::Image { mime_type, .. } = &mut message { *mime_type = "image/png".into(); }
        let prepared = PreparedMessages::prepare(&[message.clone()]).unwrap();
        assert_eq!(prepared.get(0).unwrap().mime_type, "image/png");
        drop(prepared);
        std::fs::write(&path, b"not an image").unwrap();
        assert!(PreparedMessages::prepare(&[message]).is_err());
    }

    #[test]
    fn oversize_or_nonregular_sources_fail_before_creating_clipboard_payload() {
        let fixture = Fixture::new();
        let message = fixture.file("large.bin", b"small");
        OpenOptions::new().write(true).open(fixture.0.join("large.bin")).unwrap().set_len(FILE_LIMIT + 1).unwrap();
        assert!(PreparedMessages::prepare(&[message]).is_err());
        #[cfg(unix)] {
            let original = fixture.file("real.bin", b"real bytes");
            let link = fixture.0.join("link.bin");
            std::os::unix::fs::symlink(fixture.0.join("real.bin"), &link).unwrap();
            let linked = Message::File { path: link.to_str().unwrap().into(), filename: "link.bin".into(), mime_type: "application/octet-stream".into() };
            assert!(PreparedMessages::prepare(&[linked]).is_err());
            assert!(PreparedMessages::prepare(&[original]).is_ok());
        }
    }
}
