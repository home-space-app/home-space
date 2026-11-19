//! A transaction on one user's tree. It offers the steps (lock, look up, relocate, archive,
//! bin) that the file services compose into moves and stores; what those changes mean is
//! decided there, not here.

use sqlx::{Postgres, Transaction};

use crate::{Db, DbError, UserId};

/// An open transaction on the tree of one owner. Every step is scoped to that owner; nothing
/// is kept unless [`Tx::commit`] is called.
pub struct Tx {
    pub(crate) conn: Transaction<'static, Postgres>,
    pub(crate) owner: UserId,
}

impl Db {
    /// Starts a transaction on `owner`'s tree, recording this handle's device as the maker of
    /// the node changes made in it.
    pub async fn transaction(&self, owner: UserId) -> Result<Tx, DbError> {
        Ok(Tx {
            conn: self.begin().await?,
            owner,
        })
    }
}

impl Tx {
    /// The owner of the tree this transaction works on.
    pub fn owner(&self) -> UserId {
        self.owner
    }

    /// Keeps every change made in the transaction.
    pub async fn commit(self) -> Result<(), DbError> {
        self.conn.commit().await?;
        Ok(())
    }
}
