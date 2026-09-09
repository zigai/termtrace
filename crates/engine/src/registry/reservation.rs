use super::*;

impl SessionRegistry {
    /// Register a new alias without replacing an existing binding or reservation.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid alias, insecure or unreadable session,
    /// schema/identity mismatch, or existing alias reservation/binding.
    pub fn register(&self, alias: &str, session: &Path) -> Result<RegistryEntry> {
        let _lock = self.lock()?;

        validate_alias(alias)?;

        if self.entry_path(alias).exists() {
            return Err(registry_error(
                "register session alias",
                "alias is already reserved or registered",
            ));
        }

        let entry = self.entry_for(alias, session)?;
        let encoded = self.encode_entry(&entry)?;
        write_secure(&self.entry_path(alias), &encoded)?;
        return Ok(entry);
    }

    /// Atomically reserve an alias for one pending detached session.
    ///
    /// Expired reservations are reclaimed while holding the registry lock.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid aliases, zero leases, or a live reservation/binding.
    pub fn reserve(
        &self,
        alias: &str,
        pending_session: &Path,
        lease: Duration,
    ) -> Result<AliasReservation> {
        validate_alias(alias)?;

        if lease.is_zero() {
            return Err(registry_error(
                "reserve session alias",
                "reservation lease must be greater than zero",
            ));
        }

        let _lock = self.lock()?;
        let now = system_time_unix_ns()?;
        let path = self.entry_path(alias);
        if path.exists() {
            match read_registry_record_nofollow(&path)? {
                RegistryRecord::Reservation(reservation)
                    if valid_reservation(alias, &reservation)
                        && reservation.lease_expires_unix_ns <= now =>
                {
                    fs::remove_file(&path).map_err(|source| {
                        return Error::Filesystem {
                            path: path.clone(),
                            source,
                        };
                    })?;

                    sync_parent_directory(&path)?;
                }
                RegistryRecord::Reservation(reservation)
                    if !valid_reservation(alias, &reservation) =>
                {
                    return Err(registry_error(
                        "reserve session alias",
                        "existing alias reservation is incompatible",
                    ));
                }
                RegistryRecord::Reservation(_) | RegistryRecord::Entry(_) => {
                    return Err(registry_error(
                        "reserve session alias",
                        "alias is already reserved or registered",
                    ));
                }
            }
        }

        let lease_expires_unix_ns = now.saturating_add(lease.as_nanos());
        let reservation = AliasReservation {
            format: REGISTRY_RESERVATION_FORMAT.to_owned(),
            version: REGISTRY_RESERVATION_VERSION,
            alias: alias.to_owned(),
            owner_token: Uuid::now_v7(),
            created_unix_ns: now,
            lease_expires_unix_ns,
            pending_session: ByteString::from(pending_session.as_os_str().as_bytes()),
        };
        let encoded = self.encode_reservation(&reservation)?;
        write_secure(&path, &encoded)?;
        return Ok(reservation);
    }

    /// Atomically replace an owned reservation with its validated session binding.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the reservation when ownership changed or
    /// pending session evidence is not ready for publication.
    pub fn promote(&self, reservation: &AliasReservation) -> Result<RegistryEntry> {
        let _lock = self.lock()?;
        let path = self.entry_path(&reservation.alias);
        let RegistryRecord::Reservation(current) = read_registry_record_nofollow(&path)? else {
            return Err(registry_error(
                "promote session alias",
                "alias is no longer reserved",
            ));
        };

        if !valid_reservation(&reservation.alias, &current)
            || current.owner_token != reservation.owner_token
            || current.created_unix_ns != reservation.created_unix_ns
            || current.lease_expires_unix_ns != reservation.lease_expires_unix_ns
            || current.pending_session != reservation.pending_session
        {
            return Err(registry_error(
                "promote session alias",
                "alias reservation owner changed before promotion",
            ));
        }

        if current.lease_expires_unix_ns <= system_time_unix_ns()? {
            return Err(registry_error(
                "promote session alias",
                "alias reservation lease expired before promotion",
            ));
        }

        let entry = self.entry_for(&reservation.alias, &reservation.session_path())?;
        let encoded = self.encode_entry(&entry)?;
        let temporary =
            self.directory
                .join(format!(".{}.{}.tmp", reservation.alias, Uuid::now_v7()));

        write_secure(&temporary, &encoded)?;

        if let Err(source) = fs::rename(&temporary, &path) {
            let _cleanup = fs::remove_file(&temporary);
            return Err(Error::Filesystem { path, source });
        }

        sync_parent_directory(&path)?;
        return Ok(entry);
    }

    /// Release a reservation only when its owner token still matches.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the alias when ownership changed.
    pub fn release(&self, reservation: &AliasReservation) -> Result<()> {
        let _lock = self.lock()?;
        let path = self.entry_path(&reservation.alias);
        let RegistryRecord::Reservation(current) = read_registry_record_nofollow(&path)? else {
            return Err(registry_error(
                "release session alias",
                "alias is no longer reserved",
            ));
        };

        if !valid_reservation(&reservation.alias, &current)
            || current.owner_token != reservation.owner_token
        {
            return Err(registry_error(
                "release session alias",
                "alias reservation owner changed before release",
            ));
        }

        fs::remove_file(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;

        sync_parent_directory(&path)?;
        return Ok(());
    }
}

pub(super) fn validate_alias(alias: &str) -> Result<()> {
    if alias.is_empty()
        || alias.len() > 64
        || !alias.bytes().all(|byte| {
            return byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_';
        })
        || !alias
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(registry_error(
            "validate session alias",
            "alias must be 1-64 ASCII letters, digits, '-' or '_', beginning with a letter or digit",
        ));
    }
    return Ok(());
}

pub(super) fn system_time_unix_ns() -> Result<u128> {
    return Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| return Error::ClockBeforeEpoch)?
        .as_nanos());
}

pub(super) fn valid_reservation(alias: &str, reservation: &AliasReservation) -> bool {
    return reservation.format == REGISTRY_RESERVATION_FORMAT
        && reservation.version == REGISTRY_RESERVATION_VERSION
        && reservation.alias == alias
        && reservation.created_unix_ns < reservation.lease_expires_unix_ns
        && !reservation.pending_session.as_bytes().is_empty();
}
