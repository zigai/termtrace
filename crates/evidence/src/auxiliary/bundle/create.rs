use std::{
    collections::BTreeSet,
    fs::File,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use nix::{
    errno::Errno,
    fcntl::{renameat2, RenameFlags},
};
use termtrace_model::{artifact::bundle::BundleManifest, artifact::metadata::ArtifactMember};
use uuid::Uuid;

use crate::{
    artifact::BudgetWriter,
    auxiliary::bundle::{
        create_private_directory, create_private_parents, secure_file, sync_directory, sync_parent,
        validate_relative_path, BundleIdentity, BundleMemberMetadata, BUNDLE_MANIFEST_NAME,
        MAX_BUNDLE_MANIFEST_BYTES, MAX_BUNDLE_MEMBERS,
    },
    storage::{Error, Result},
};

/// Transactional builder for a new portable bundle directory.
pub struct BundleDraft {
    destination: PathBuf,
    staging: PathBuf,
    identity: BundleIdentity,
    members: Vec<ArtifactMember>,
    paths: BTreeSet<String>,
    committed: bool,
    remaining_bytes: u64,
}

impl BundleDraft {
    /// Create a private staging directory for a bundle destination.
    ///
    /// # Errors
    ///
    /// Returns an error when the destination exists or staging cannot be created.
    pub fn create(
        destination: impl Into<PathBuf>,
        identity: BundleIdentity,
        max_bytes: Option<u64>,
    ) -> Result<Self> {
        let destination = destination.into();
        let parent = destination
            .parent()
            .ok_or_else(|| return Error::invalid("bundle path has no parent"))?;
        let name = destination
            .file_name()
            .ok_or_else(|| return Error::invalid("bundle path has no basename"))?;

        if !parent.is_dir() {
            return Err(Error::invalid("bundle parent directory does not exist"));
        }

        if destination.exists() {
            return Err(Error::DestinationExists {
                path: destination.clone(),
            });
        }

        let staging = parent.join(format!(
            ".{}.{}.tmp",
            name.to_string_lossy(),
            Uuid::now_v7()
        ));

        create_private_directory(&staging)?;

        return Ok(Self {
            destination,
            staging,
            identity,
            members: Vec::new(),
            paths: BTreeSet::new(),
            committed: false,
            remaining_bytes: max_bytes.unwrap_or(u64::MAX),
        });
    }

    /// Start streaming one new member into the draft.
    ///
    /// The member is not added to the manifest until its writer is finished.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe or duplicate paths, invalid metadata, capacity exhaustion, or
    /// file creation failure.
    pub fn start_member<'draft>(
        &'draft mut self,
        relative: impl AsRef<Path>,
        metadata: BundleMemberMetadata,
    ) -> Result<BundleMemberWriter<'draft>> {
        if self.members.len() >= MAX_BUNDLE_MEMBERS {
            return Err(Error::invalid("bundle contains too many members"));
        }

        if metadata.media_type.is_empty() || metadata.relationship.is_empty() {
            return Err(Error::invalid(
                "bundle member media type and relationship must be non-empty",
            ));
        }

        let relative = relative.as_ref();
        let path = validate_relative_path(relative)?;
        if !self.paths.insert(path.clone()) {
            return Err(Error::invalid(format!(
                "bundle contains duplicate member `{path}`"
            )));
        }

        create_private_parents(&self.staging, relative)?;
        let destination = self.staging.join(relative);
        let writer = match secure_file(&destination) {
            Ok(file) => BufWriter::new(file),
            Err(error) => {
                self.paths.remove(&path);

                return Err(error);
            }
        };

        return Ok(BundleMemberWriter {
            draft: self,
            writer: Some(writer),
            destination,
            path,
            metadata,
            hasher: blake3::Hasher::new(),
            byte_length: 0,
            finished: false,
        });
    }

    /// Copy one member from a reader and finalize its metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when the member cannot be created, copied, or synced.
    pub fn add_member_from_reader(
        &mut self,
        relative: impl AsRef<Path>,
        metadata: BundleMemberMetadata,
        reader: &mut impl io::Read,
    ) -> Result<ArtifactMember> {
        let mut member = self.start_member(relative, metadata)?;
        io::copy(reader, &mut member)
            .map_err(|source| return Error::filesystem(&member.destination, source))?;

        return member.finish();
    }

    /// Add and finalize one in-memory member.
    ///
    /// # Errors
    ///
    /// Returns an error when the member cannot be created, written, or synced.
    pub fn add_member_bytes(
        &mut self,
        relative: impl AsRef<Path>,
        metadata: BundleMemberMetadata,
        bytes: &[u8],
    ) -> Result<ArtifactMember> {
        return self.add_member_from_reader(relative, metadata, &mut io::Cursor::new(bytes));
    }

    /// Private staging directory containing the in-progress bundle.
    #[must_use]
    pub fn staging_path(&self) -> &Path {
        return &self.staging;
    }

    /// Write the manifest and atomically publish the completed bundle.
    ///
    /// # Errors
    ///
    /// Returns an error when the manifest exceeds its bound or publication cannot be completed
    /// durably.
    pub fn commit(mut self) -> Result<BundleManifest> {
        self.members
            .sort_by(|left, right| return left.path.cmp(&right.path));
        let manifest = self.identity.manifest(self.members.clone());
        let manifest_path = self.staging.join(BUNDLE_MANIFEST_NAME);
        let mut manifest_file = secure_file(&manifest_path)?;
        {
            let mut writer = BudgetWriter::new(
                &mut manifest_file,
                self.remaining_bytes.min(MAX_BUNDLE_MANIFEST_BYTES),
            );
            serde_json::to_writer_pretty(&mut writer, &manifest).map_err(|source| {
                return Error::InvalidJson {
                    path: manifest_path.clone(),
                    json_path: "$".to_owned(),
                    source,
                };
            })?;
            writer
                .write_all(b"\n")
                .map_err(|source| return Error::filesystem(&manifest_path, source))?;
        }

        manifest_file
            .sync_all()
            .map_err(|source| return Error::filesystem(&manifest_path, source))?;

        sync_directory(&self.staging)?;

        let parent = self
            .destination
            .parent()
            .ok_or_else(|| return Error::invalid("bundle path has no parent"))?;
        let staging_name = self
            .staging
            .file_name()
            .ok_or_else(|| return Error::invalid("staging path has no basename"))?;
        let destination_name = self
            .destination
            .file_name()
            .ok_or_else(|| return Error::invalid("bundle path has no basename"))?;

        let directory =
            File::open(parent).map_err(|source| return Error::filesystem(parent, source))?;
        renameat2(
            &directory,
            staging_name,
            &directory,
            destination_name,
            RenameFlags::RENAME_NOREPLACE,
        )
        .map_err(|error| {
            if error == Errno::EEXIST {
                return Error::DestinationExists {
                    path: self.destination.clone(),
                };
            }

            return Error::filesystem(
                &self.destination,
                io::Error::from_raw_os_error(error as i32),
            );
        })?;

        sync_parent(&self.destination)?;
        self.committed = true;

        return Ok(manifest);
    }
}

impl Drop for BundleDraft {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs_err::remove_dir_all(&self.staging);
        }
    }
}

/// Streaming writer for one bundle member.
pub struct BundleMemberWriter<'draft> {
    draft: &'draft mut BundleDraft,
    writer: Option<BufWriter<File>>,
    destination: PathBuf,
    path: String,
    metadata: BundleMemberMetadata,
    hasher: blake3::Hasher,
    byte_length: u64,
    finished: bool,
}

impl BundleMemberWriter<'_> {
    /// Sync the member and add its measured metadata to the draft.
    ///
    /// # Errors
    ///
    /// Returns an error when the member is already finished or cannot be flushed and synced.
    pub fn finish(mut self) -> Result<ArtifactMember> {
        let mut writer = self
            .writer
            .take()
            .ok_or_else(|| return Error::invalid("bundle member writer is already finished"))?;
        writer
            .flush()
            .and_then(|()| return writer.get_ref().sync_all())
            .map_err(|source| return Error::filesystem(&self.destination, source))?;

        sync_parent(&self.destination)?;

        let member = ArtifactMember {
            path: self.path.clone(),
            byte_length: self.byte_length,
            blake3: self.hasher.finalize().to_hex().to_string(),
            media_type: self.metadata.media_type.clone(),
            schema_type: self.metadata.schema_type.clone(),
            sensitivity: self.metadata.sensitivity,
            relationship: self.metadata.relationship.clone(),
        };
        self.draft.members.push(member.clone());
        self.finished = true;

        return Ok(member);
    }
}

impl Write for BundleMemberWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if u64::try_from(buf.len()).unwrap_or(u64::MAX) > self.draft.remaining_bytes {
            return Err(io::Error::other("bundle exceeds its byte budget"));
        }

        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| return io::Error::other("bundle member writer is already finished"))?;
        let count = writer.write(buf)?;
        self.draft.remaining_bytes -= u64::try_from(count).unwrap_or(u64::MAX);
        let written = buf.get(..count).ok_or_else(|| {
            return io::Error::other("bundle member writer returned an invalid count");
        })?;
        self.hasher.update(written);
        self.byte_length = self
            .byte_length
            .checked_add(u64::try_from(count).unwrap_or(u64::MAX))
            .ok_or_else(|| return io::Error::other("bundle member size overflow"))?;

        return Ok(count);
    }

    fn flush(&mut self) -> io::Result<()> {
        return self
            .writer
            .as_mut()
            .ok_or_else(|| return io::Error::other("bundle member writer is already finished"))?
            .flush();
    }
}

impl Drop for BundleMemberWriter<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }

        self.writer.take();
        self.draft.remaining_bytes = self.draft.remaining_bytes.saturating_add(self.byte_length);
        let _ = fs_err::remove_file(&self.destination);
        self.draft.paths.remove(&self.path);
    }
}
