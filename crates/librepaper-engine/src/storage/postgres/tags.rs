//! Project tags: named labels one account puts on the projects it can see.
//!
//! A tag belongs to one account, so a collaborator on a shared project keeps
//! their own tags on it and nobody else sees them. A tag is one person's note
//! about a document, like a mark, so nothing here grants any access: the
//! listing that reads tags joins them to the documents the caller may already
//! see, and a tag on a document that was since trashed or unshared simply
//! stops showing.
//!
//! Favorites is the one built-in tag. It has a row only once the account has
//! starred something, and its name and colour never change.

use uuid::Uuid;

use super::{Error, PostgresCatalog, Result};

/// The colours a tag may take. Clients draw them by key, so a new colour is a
/// new key and never a new spelling of an old one.
pub const COLOURS: [&str; 9] = [
    "gray", "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink",
];

/// How many tags one account may create. The built-in tag does not count.
pub const MAX_TAGS: i64 = 100;

pub const FAVORITES_NAME: &str = "Favorites";
pub const FAVORITES_COLOUR: &str = "yellow";

const NAME_MAX_CHARS: usize = 40;

#[derive(Clone, Debug)]
pub struct TagRecord {
    pub id: Uuid,
    pub builtin: bool,
    pub name: String,
    pub colour: String,
}

/// One tag on one document, for the listing to decorate its rows with.
#[derive(Clone, Debug)]
pub struct DocumentTagRecord {
    pub document_id: Uuid,
    pub tag_id: Uuid,
    pub builtin: bool,
}

/// Trims a tag name and checks it. Names are unique ignoring case, which the
/// index enforces; the reserved name is refused here because it belongs to
/// the built-in tag and is not a clash between two of the account's own.
pub fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Invalid("give the tag a name".into()));
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Err(Error::Invalid(format!(
            "a tag name is at most {NAME_MAX_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::Invalid(
            "a tag name cannot contain control characters".into(),
        ));
    }
    if name.eq_ignore_ascii_case(FAVORITES_NAME) {
        return Err(Error::Invalid(format!(
            "{FAVORITES_NAME} is the built-in tag's name"
        )));
    }
    Ok(name.to_owned())
}

pub fn validate_colour(colour: &str) -> Result<()> {
    if COLOURS.contains(&colour) {
        Ok(())
    } else {
        Err(Error::Invalid("that colour is not one of the palette".into()))
    }
}

/// A name already taken is a conflict, not a bad request: the request was
/// well formed and the account already has the tag.
fn duplicate_as_conflict(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::Database(ref database) if database.is_unique_violation() => {
            Error::Conflict("a tag with that name already exists".into())
        }
        other => Error::Database(other),
    }
}

impl PostgresCatalog {
    /// Every tag this account holds, Favorites first and the rest by name.
    pub async fn list_tags(&self, account_id: Uuid) -> Result<Vec<TagRecord>> {
        sqlx::query_as!(
            TagRecord,
            r#"SELECT id, builtin IS NOT NULL AS "builtin!", name, colour
               FROM tags
               WHERE account_id=$1
               ORDER BY builtin IS NULL, lower(name)"#,
            account_id,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// The account's tag with this id. Another account's tag is not found,
    /// the same as one that never existed.
    pub async fn tag_for(&self, account_id: Uuid, tag_id: Uuid) -> Result<TagRecord> {
        sqlx::query_as!(
            TagRecord,
            r#"SELECT id, builtin IS NOT NULL AS "builtin!", name, colour
               FROM tags
               WHERE id=$1 AND account_id=$2"#,
            tag_id,
            account_id,
        )
        .fetch_optional(&self.pool)
        .await?
        .ok_or(Error::NotFound)
    }

    /// The id of the account's Favorites row, or none if it has never starred
    /// anything. Taking a star off never needs the row to exist.
    pub async fn favorites_tag(&self, account_id: Uuid) -> Result<Option<Uuid>> {
        sqlx::query_scalar!(
            "SELECT id FROM tags WHERE account_id=$1 AND builtin='favorites'",
            account_id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// The id of the account's Favorites row, creating it the first time.
    pub async fn ensure_favorites(&self, account_id: Uuid) -> Result<Uuid> {
        sqlx::query!(
            "INSERT INTO tags(id,account_id,name,colour,builtin)
             VALUES($1,$2,$3,$4,'favorites')
             ON CONFLICT(account_id,builtin) WHERE builtin IS NOT NULL DO NOTHING",
            Uuid::now_v7(),
            account_id,
            FAVORITES_NAME,
            FAVORITES_COLOUR,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query_scalar!(
            "SELECT id FROM tags WHERE account_id=$1 AND builtin='favorites'",
            account_id,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(Error::from)
    }

    /// A new tag of the account's own. The limit is checked in the same
    /// statement as the insert, so two creates cannot both claim the last
    /// place.
    pub async fn create_tag(
        &self,
        account_id: Uuid,
        name: &str,
        colour: &str,
    ) -> Result<TagRecord> {
        let name = validate_name(name)?;
        validate_colour(colour)?;
        let id = Uuid::now_v7();
        let inserted = sqlx::query_scalar!(
            "INSERT INTO tags(id,account_id,name,colour)
             SELECT $1::uuid,$2::uuid,$3::text,$4::text
             WHERE (SELECT count(*) FROM tags
                    WHERE account_id=$2::uuid AND builtin IS NULL) < $5::bigint
             RETURNING id",
            id,
            account_id,
            name,
            colour,
            MAX_TAGS,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(duplicate_as_conflict)?;
        if inserted.is_none() {
            return Err(Error::Invalid(format!(
                "an account can have at most {MAX_TAGS} tags"
            )));
        }
        Ok(TagRecord {
            id,
            builtin: false,
            name,
            colour: colour.to_owned(),
        })
    }

    /// Rename or recolour one of the account's own tags. The built-in tag is
    /// refused here as well as in the store, so no caller can change it.
    pub async fn update_tag(
        &self,
        account_id: Uuid,
        tag_id: Uuid,
        name: Option<&str>,
        colour: Option<&str>,
    ) -> Result<TagRecord> {
        let current = self.tag_for(account_id, tag_id).await?;
        if current.builtin {
            return Err(built_in_refusal());
        }
        let name = name.map(validate_name).transpose()?;
        if let Some(colour) = colour {
            validate_colour(colour)?;
        }
        let updated = sqlx::query!(
            "UPDATE tags SET name=COALESCE($3::text,name), colour=COALESCE($4::text,colour)
             WHERE id=$1 AND account_id=$2 AND builtin IS NULL
             RETURNING name, colour",
            tag_id,
            account_id,
            name,
            colour,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(duplicate_as_conflict)?
        .ok_or(Error::NotFound)?;
        Ok(TagRecord {
            id: tag_id,
            builtin: false,
            name: updated.name,
            colour: updated.colour,
        })
    }

    /// Delete one of the account's own tags. Projects are never touched: the
    /// links go, and the documents keep everything else they hold.
    pub async fn delete_tag(&self, account_id: Uuid, tag_id: Uuid) -> Result<()> {
        if self.tag_for(account_id, tag_id).await?.builtin {
            return Err(built_in_refusal());
        }
        sqlx::query!(
            "DELETE FROM tags WHERE id=$1 AND account_id=$2 AND builtin IS NULL",
            tag_id,
            account_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Put a tag on a document, or take it off. Both are idempotent, so a
    /// repeated request leaves the same state as the first one did.
    pub async fn set_document_tag(
        &self,
        account_id: Uuid,
        document_id: Uuid,
        tag_id: Uuid,
        on: bool,
    ) -> Result<()> {
        self.tag_for(account_id, tag_id).await?;
        if on {
            sqlx::query!(
                "INSERT INTO document_tags(tag_id,document_id) VALUES($1,$2)
                 ON CONFLICT(tag_id,document_id) DO NOTHING",
                tag_id,
                document_id,
            )
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query!(
                "DELETE FROM document_tags WHERE tag_id=$1 AND document_id=$2",
                tag_id,
                document_id,
            )
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Every tag this account has put on the documents named, in one query.
    /// The listing asks for one page of documents at a time and decorates it
    /// with this, rather than joining tags into the listing query.
    pub async fn tags_for_documents(
        &self,
        account_id: Uuid,
        document_ids: &[Uuid],
    ) -> Result<Vec<DocumentTagRecord>> {
        if document_ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as!(
            DocumentTagRecord,
            r#"SELECT dt.document_id, dt.tag_id, t.builtin IS NOT NULL AS "builtin!"
               FROM document_tags dt
               JOIN tags t ON t.id = dt.tag_id
               WHERE t.account_id=$1 AND dt.document_id=ANY($2)
               ORDER BY t.builtin IS NULL, lower(t.name)"#,
            account_id,
            document_ids,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::from)
    }
}

/// The refusal for any write to the built-in tag, shared with the store so
/// the message is said once.
pub fn built_in_refusal() -> Error {
    Error::Invalid(format!(
        "{FAVORITES_NAME} cannot be renamed, recoloured or deleted"
    ))
}

#[cfg(test)]
#[path = "tags_regression_tests.rs"]
mod tags_regression_tests;
