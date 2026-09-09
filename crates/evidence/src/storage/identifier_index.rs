//! Private disk indexes; canonical JSONL remains authoritative.

use super::{Error, Result};
use rusqlite::{params, Connection, OptionalExtension as _};
use std::{collections::HashSet, io, path::Path};
use uuid::Uuid;

pub(crate) fn index_error(source: rusqlite::Error) -> Error {
    return Error::filesystem(Path::new("<identifier-index>"), io::Error::other(source));
}

/// A small set stays in memory; lifetime history spills to a temporary disk database.
#[derive(Default)]
pub(crate) struct IdentifierSet {
    memory: HashSet<Uuid>,
    disk: Option<Connection>,
}

impl IdentifierSet {
    pub(crate) fn insert(&mut self, id: Uuid) -> Result<bool> {
        if self.disk.is_none() && self.memory.len() < 4096 {
            return Ok(self.memory.insert(id));
        }

        if self.disk.is_none() {
            // SQLite's empty filename creates a private temporary on-disk database,
            // removed when this connection closes. Validation needs no durable index.
            let mut disk = Connection::open("").map_err(index_error)?;
            disk.execute_batch("PRAGMA cache_size=-1024; PRAGMA temp_store=FILE; PRAGMA journal_mode=OFF; CREATE TABLE ids (id BLOB PRIMARY KEY) WITHOUT ROWID;").map_err(index_error)?;

            {
                let transaction = disk.transaction().map_err(index_error)?;
                for existing in &self.memory {
                    transaction
                        .execute(
                            "INSERT INTO ids VALUES (?1)",
                            [existing.as_bytes().as_slice()],
                        )
                        .map_err(index_error)?;
                }

                transaction.commit().map_err(index_error)?;
            }

            self.memory.clear();
            self.memory.shrink_to_fit();
            self.disk = Some(disk);
        }

        let disk = self
            .disk
            .as_ref()
            .ok_or_else(|| return Error::invalid("identifier spill did not create its index"))?;

        return disk
            .execute(
                "INSERT OR IGNORE INTO ids VALUES (?1)",
                [id.as_bytes().as_slice()],
            )
            .map(|count| return count != 0)
            .map_err(index_error);
    }
}

#[derive(Debug)]
pub(crate) struct RequestIndex {
    connection: Connection,
}

pub(crate) struct RequestPosition {
    pub(crate) offset: u64,
    pub(crate) length: u64,
    pub(crate) sequence: u64,
    pub(crate) previous_hash: String,
    pub(crate) hash: String,
}

impl RequestIndex {
    pub(crate) fn create(path: &Path) -> Result<Self> {
        super::artifact::secure_file(path)?;
        let connection = Connection::open(path).map_err(index_error)?;
        connection.execute_batch("PRAGMA cache_size=-1024; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
            CREATE TABLE operations (id BLOB PRIMARY KEY) WITHOUT ROWID;
            CREATE TABLE requests (id BLOB PRIMARY KEY, offset INTEGER NOT NULL, length INTEGER NOT NULL, sequence INTEGER NOT NULL, previous_hash TEXT NOT NULL, hash TEXT NOT NULL) WITHOUT ROWID;").map_err(index_error)?;

        return Ok(Self { connection });
    }

    pub(crate) fn reserve(&mut self, ids: &[Uuid]) -> Result<bool> {
        let transaction = self.connection.transaction().map_err(index_error)?;

        for id in ids {
            let count = transaction
                .execute(
                    "INSERT OR IGNORE INTO operations VALUES (?1)",
                    [id.as_bytes().as_slice()],
                )
                .map_err(index_error)?;

            if count == 0 {
                // Dropping this uncommitted transaction rolls back the entire batch.
                return Ok(false);
            }
        }

        transaction.commit().map_err(index_error)?;
        return Ok(true);
    }

    pub(crate) fn insert(&self, id: Uuid, position: &RequestPosition) -> Result<()> {
        self.connection
            .execute(
                "INSERT INTO requests VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id.as_bytes().as_slice(),
                    position.offset,
                    position.length,
                    position.sequence,
                    position.previous_hash,
                    position.hash
                ],
            )
            .map_err(index_error)?;

        return Ok(());
    }

    pub(crate) fn lookup(&self, id: Uuid) -> Result<Option<RequestPosition>> {
        return self
            .connection
            .query_row(
                "SELECT offset, length, sequence, previous_hash, hash FROM requests WHERE id=?1",
                [id.as_bytes().as_slice()],
                |row| {
                    return Ok(RequestPosition {
                        offset: row.get(0)?,
                        length: row.get(1)?,
                        sequence: row.get(2)?,
                        previous_hash: row.get(3)?,
                        hash: row.get(4)?,
                    });
                },
            )
            .optional()
            .map_err(index_error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spilled_identifiers_still_reject_early_duplicates() {
        let mut ids = IdentifierSet::default();
        for id in 0..8192 {
            assert!(ids.insert(Uuid::from_u128(id)).expect("insert"));
        }

        assert!(!ids.insert(Uuid::nil()).expect("duplicate"));
        assert!(ids.memory.is_empty());
        assert!(ids.disk.is_some());
    }
}
