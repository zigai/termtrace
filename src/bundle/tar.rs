use super::*;

/// Publish one deterministic Zstandard-compressed USTAR bundle archive.
pub fn archive(
    directory: &Path,
    destination: &Path,
    policy: &BundleVerificationPolicy,
) -> Result<()> {
    verify(directory, policy)?;
    if destination.exists() {
        return Err(Error::ArtifactExists {
            path: destination.to_path_buf(),
        });
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return bundle_error("archive path has no parent"))?;
    if !parent.is_dir() {
        return Err(bundle_error("archive parent directory does not exist"));
    }
    let staging = parent.join(format!(".bundle-archive-{}.tmp", Uuid::now_v7()));
    let file = secure_file(&staging)?;
    let mut encoder = zstd::stream::write::Encoder::new(file, 9).map_err(|source| {
        return Error::Filesystem {
            path: staging.clone(),
            source,
        };
    })?;
    encoder.include_checksum(true).map_err(|source| {
        return Error::Filesystem {
            path: staging.clone(),
            source,
        };
    })?;
    let mut files = Vec::new();
    collect_bundle_files(directory, directory, &mut files)?;
    files.sort();
    for relative in files {
        write_tar_file(&mut encoder, directory, &relative)?;
    }
    encoder.write_all(&[0_u8; 1024]).map_err(|source| {
        return Error::Filesystem {
            path: staging.clone(),
            source,
        };
    })?;
    let file = encoder.finish().map_err(|source| {
        return Error::Filesystem {
            path: staging.clone(),
            source,
        };
    })?;
    file.sync_all().map_err(|source| {
        return Error::Filesystem {
            path: staging.clone(),
            source,
        };
    })?;
    if let Err(error) = commit_directory(parent, &staging, destination) {
        let _cleanup = fs_err::remove_file(&staging);
        return Err(error);
    }
    return Ok(());
}

/// Verify a deterministic compressed archive after bounded safe extraction.
pub fn verify_archive(archive: &Path, policy: &BundleVerificationPolicy) -> Result<BundleManifest> {
    let parent = archive
        .parent()
        .ok_or_else(|| return bundle_error("archive path has no parent"))?;
    let staging = parent.join(format!(".bundle-verify-{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;
    let result = (|| {
        extract_archive(archive, &staging)?;
        return verify(&staging, policy);
    })();
    let _cleanup = fs_err::remove_dir_all(&staging);
    return result;
}

pub(super) fn collect_bundle_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs_err::read_dir(directory).map_err(|source| {
        return Error::Filesystem {
            path: directory.to_path_buf(),
            source,
        };
    })? {
        let entry = entry.map_err(|source| {
            return Error::Filesystem {
                path: directory.to_path_buf(),
                source,
            };
        })?;
        let file_type = entry.file_type().map_err(|source| {
            return Error::Filesystem {
                path: entry.path(),
                source,
            };
        })?;
        if file_type.is_symlink()
            || file_type.is_block_device()
            || file_type.is_char_device()
            || file_type.is_fifo()
            || file_type.is_socket()
        {
            return Err(bundle_error(
                "bundle archive source contains a special file",
            ));
        }
        if file_type.is_dir() {
            collect_bundle_files(root, &entry.path(), files)?;
        } else {
            files.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|_error| return bundle_error("bundle path escaped its root"))?
                    .to_path_buf(),
            );
        }
    }
    return Ok(());
}

pub(super) fn write_tar_file(
    writer: &mut impl io::Write,
    root: &Path,
    relative: &Path,
) -> Result<()> {
    let path = relative
        .to_str()
        .ok_or_else(|| return bundle_error("bundle archive paths must be UTF-8"))?;
    let source = root.join(relative);
    let size = fs_err::metadata(&source)
        .map_err(|source_error| {
            return Error::Filesystem {
                path: source.clone(),
                source: source_error,
            };
        })?
        .len();
    let mut header = [0_u8; 512];
    write_tar_path(&mut header, path)?;
    write_octal(&mut header, 100, 8, 0o600)?;
    write_octal(&mut header, 108, 8, 0)?;
    write_octal(&mut header, 116, 8, 0)?;
    write_octal(&mut header, 124, 12, size)?;
    write_octal(&mut header, 136, 12, 0)?;
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|byte| return u64::from(*byte)).sum();
    write_checksum(&mut header, checksum)?;
    writer.write_all(&header).map_err(|source_error| {
        return Error::Filesystem {
            path: source.clone(),
            source: source_error,
        };
    })?;
    let mut input = fs_err::File::open(&source).map_err(|source_error| {
        return Error::Filesystem {
            path: source.clone(),
            source: source_error,
        };
    })?;
    io::copy(&mut input, writer).map_err(|source_error| {
        return Error::Filesystem {
            path: source.clone(),
            source: source_error,
        };
    })?;
    let padding = (512 - size % 512) % 512;
    writer
        .write_all(&vec![0_u8; usize::try_from(padding).unwrap_or(511)])
        .map_err(|source_error| {
            return Error::Filesystem {
                path: source,
                source: source_error,
            };
        })?;
    return Ok(());
}

pub(super) fn write_tar_path(header: &mut [u8; 512], path: &str) -> Result<()> {
    if path.len() <= 100 {
        header
            .get_mut(..path.len())
            .ok_or_else(|| return bundle_error("tar path header overflowed"))?
            .copy_from_slice(path.as_bytes());
        return Ok(());
    }
    let split = path
        .char_indices()
        .filter(|(_, character)| return *character == '/')
        .map(|(index, _)| return index)
        .rev()
        .find(|index| return *index <= 155 && path.len().saturating_sub(*index + 1) <= 100)
        .ok_or_else(|| return bundle_error("bundle path exceeds USTAR limits"))?;
    let (prefix, name) = path.split_at(split);
    let name = name.trim_start_matches('/');
    header
        .get_mut(..name.len())
        .ok_or_else(|| return bundle_error("tar name header overflowed"))?
        .copy_from_slice(name.as_bytes());
    header
        .get_mut(345..345 + prefix.len())
        .ok_or_else(|| return bundle_error("tar prefix header overflowed"))?
        .copy_from_slice(prefix.as_bytes());
    return Ok(());
}

pub(super) fn write_octal(
    header: &mut [u8; 512],
    offset: usize,
    width: usize,
    value: u64,
) -> Result<()> {
    let encoded = format!("{:0width$o}\0", value, width = width.saturating_sub(1));
    let target = header
        .get_mut(offset..offset.saturating_add(width))
        .ok_or_else(|| return bundle_error("tar numeric header overflowed"))?;
    if encoded.len() != width {
        return Err(bundle_error("tar numeric value exceeds field width"));
    }
    target.copy_from_slice(encoded.as_bytes());
    return Ok(());
}

pub(super) fn write_checksum(header: &mut [u8; 512], checksum: u64) -> Result<()> {
    let encoded = format!("{checksum:06o}\0 ");
    if encoded.len() != 8 {
        return Err(bundle_error("tar checksum exceeds field width"));
    }
    header[148..156].copy_from_slice(encoded.as_bytes());
    return Ok(());
}

pub(super) fn extract_archive(archive: &Path, destination: &Path) -> Result<()> {
    let file = fs_err::File::open(archive).map_err(|source| {
        return Error::Filesystem {
            path: archive.to_path_buf(),
            source,
        };
    })?;
    let mut decoder = zstd::stream::read::Decoder::new(file).map_err(|source| {
        return Error::Filesystem {
            path: archive.to_path_buf(),
            source,
        };
    })?;
    let mut paths = BTreeSet::new();
    let mut total_size = 0_u64;
    let maximum_archive_members = u64::try_from(MAX_BUNDLE_MEMBERS)
        .unwrap_or(u64::MAX)
        .saturating_add(1); // payload members plus manifest.json
    for entry_count in 0..=maximum_archive_members {
        let mut header = [0_u8; 512];
        decoder.read_exact(&mut header).map_err(|source| {
            return Error::Filesystem {
                path: archive.to_path_buf(),
                source,
            };
        })?;
        if header.iter().all(|byte| return *byte == 0) {
            let mut terminator = [0_u8; 512];
            decoder.read_exact(&mut terminator).map_err(|source| {
                return Error::Filesystem {
                    path: archive.to_path_buf(),
                    source,
                };
            })?;
            if !terminator.iter().all(|byte| return *byte == 0) {
                return Err(bundle_error("tar archive has an invalid terminator"));
            }
            return Ok(());
        }
        if entry_count == maximum_archive_members {
            return Err(bundle_error(format!(
                "bundle archive exceeds {maximum_archive_members} members"
            )));
        }
        verify_tar_checksum(&header)?;
        let path = tar_path(&header)?;
        validate_archive_path(&path)?;
        if !paths.insert(path.clone()) {
            return Err(bundle_error("bundle archive contains a duplicate path"));
        }
        let type_flag = *header
            .get(156)
            .ok_or_else(|| return bundle_error("tar type header is absent"))?;
        if type_flag != b'0' && type_flag != 0 {
            return Err(bundle_error("bundle archive contains a non-regular member"));
        }
        let size = parse_tar_octal(
            header
                .get(124..136)
                .ok_or_else(|| return bundle_error("tar size header is absent"))?,
        )?;
        if path == Path::new("manifest.json") && size > MAX_BUNDLE_MANIFEST_BYTES {
            return Err(bundle_error(format!(
                "bundle manifest exceeds {MAX_BUNDLE_MANIFEST_BYTES} byte limit"
            )));
        }
        total_size = total_size.saturating_add(size);
        if total_size > 8 * 1024 * 1024 * 1024 {
            return Err(bundle_error("bundle archive expands beyond 8 GiB"));
        }
        let target = destination.join(&path);
        let parent = target
            .parent()
            .ok_or_else(|| return bundle_error("archive member has no parent"))?;
        fs_err::create_dir_all(parent).map_err(|source| {
            return Error::Filesystem {
                path: parent.to_path_buf(),
                source,
            };
        })?;
        let mut writer = secure_writer(&target)?;
        let copied = io::copy(&mut decoder.by_ref().take(size), &mut writer).map_err(|source| {
            return Error::Filesystem {
                path: target.clone(),
                source,
            };
        })?;
        if copied != size {
            return Err(bundle_error("bundle archive member is truncated"));
        }
        finish_writer(writer, &target)?;
        let padding = (512 - size % 512) % 512;
        let mut padding_bytes = vec![0_u8; usize::try_from(padding).unwrap_or(511)];
        decoder.read_exact(&mut padding_bytes).map_err(|source| {
            return Error::Filesystem {
                path: archive.to_path_buf(),
                source,
            };
        })?;
        if !padding_bytes.iter().all(|byte| return *byte == 0) {
            return Err(bundle_error("tar member padding is nonzero"));
        }
    }
    return Err(bundle_error("bundle archive has no terminator"));
}

pub(super) fn tar_path(header: &[u8; 512]) -> Result<PathBuf> {
    let name = tar_text(
        header
            .get(..100)
            .ok_or_else(|| return bundle_error("tar name header is absent"))?,
    )?;
    let prefix = tar_text(
        header
            .get(345..500)
            .ok_or_else(|| return bundle_error("tar prefix header is absent"))?,
    )?;
    if name.is_empty() {
        return Err(bundle_error("tar member name is empty"));
    }
    return Ok(if prefix.is_empty() {
        PathBuf::from(name)
    } else {
        PathBuf::from(prefix).join(name)
    });
}

pub(super) fn tar_text(field: &[u8]) -> Result<&str> {
    let length = field
        .iter()
        .position(|byte| return *byte == 0)
        .unwrap_or(field.len());
    return str::from_utf8(
        field
            .get(..length)
            .ok_or_else(|| return bundle_error("tar text field overflowed"))?,
    )
    .map_err(|_error| return bundle_error("tar paths must be UTF-8"));
}

pub(super) fn validate_archive_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| return !matches!(component, Component::Normal(_)))
    {
        return Err(bundle_error("bundle archive member path is unsafe"));
    }
    return Ok(());
}

pub(super) fn parse_tar_octal(field: &[u8]) -> Result<u64> {
    let value = tar_text(field)?.trim();
    if value.is_empty() {
        return Ok(0);
    }
    return u64::from_str_radix(value, 8)
        .map_err(|_error| return bundle_error("tar numeric field is invalid"));
}

pub(super) fn verify_tar_checksum(header: &[u8; 512]) -> Result<()> {
    let expected = parse_tar_octal(
        header
            .get(148..156)
            .ok_or_else(|| return bundle_error("tar checksum header is absent"))?,
    )?;
    let actual = header.iter().enumerate().fold(0_u64, |sum, (index, byte)| {
        return sum.saturating_add(if (148..156).contains(&index) {
            u64::from(b' ')
        } else {
            u64::from(*byte)
        });
    });
    if actual != expected {
        return Err(bundle_error("tar header checksum is invalid"));
    }
    return Ok(());
}
