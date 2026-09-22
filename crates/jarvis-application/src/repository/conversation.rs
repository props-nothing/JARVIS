//! The conversation and message repository ports.
//!
//! A conversation is the durable transcript container; messages are its ordered
//! entries. The message port's append is idempotent on
//! `(conversation_id, sequence)` rather than on a caller-supplied key, because the
//! schema already makes that pair unique and a second mechanism would be a second
//! answer to "is this the same message".

use crate::repository::{RepositoryError, RepositoryFuture};
use jarvis_domain::ids::{ConversationId, MessageId, PrincipalId, WorkspaceId};
use jarvis_domain::model::stream::Role;
use jarvis_domain::time::UtcTimestamp;

/// The largest accepted conversation title or message content.
///
/// A bound exists because both reach operator output and, for content, a persisted
/// row. Large content belongs in an artifact with relational metadata rather than
/// in a column, so the bound is generous but finite.
pub const MAX_TITLE_BYTES: usize = 512;
/// The largest accepted inline message content.
pub const MAX_CONTENT_BYTES: usize = 64 * 1024;

/// The fields needed to create a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConversation {
    /// The conversation to create.
    pub id: ConversationId,
    /// The owning workspace. Scope is a parameter, never inferred.
    pub workspace_id: WorkspaceId,
    /// The user that owns it.
    pub owner_user_id: PrincipalId,
    /// A bounded title, or `None` for an untitled conversation.
    pub title: Option<String>,
    /// The channel the conversation started on.
    pub channel_origin: String,
    /// The instant it was created.
    pub created_at: UtcTimestamp,
}

impl NewConversation {
    /// Builds a conversation creation request.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when the title exceeds its bound or
    /// `channel_origin` is empty, since both would otherwise reach a persisted row.
    pub fn new(
        id: ConversationId,
        workspace_id: WorkspaceId,
        owner_user_id: PrincipalId,
        title: Option<String>,
        channel_origin: String,
        created_at: UtcTimestamp,
    ) -> Result<Self, RepositoryError> {
        if title
            .as_ref()
            .is_some_and(|value| value.len() > MAX_TITLE_BYTES || value.contains('\0'))
        {
            return Err(RepositoryError::Conflict { what: "title" });
        }
        if channel_origin.is_empty() || channel_origin.contains('\0') {
            return Err(RepositoryError::Conflict {
                what: "channel_origin",
            });
        }
        Ok(Self {
            id,
            workspace_id,
            owner_user_id,
            title,
            channel_origin,
            created_at,
        })
    }
}

/// A loaded conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConversation {
    /// The conversation identifier.
    pub id: ConversationId,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
    /// The owner.
    pub owner_user_id: PrincipalId,
    /// The title, when set.
    pub title: Option<String>,
    /// Whether the conversation is archived.
    pub archived: bool,
    /// The instant it was created.
    pub created_at: UtcTimestamp,
    /// The instant it last changed.
    pub updated_at: UtcTimestamp,
}

/// One message to append.
///
/// Built as a struct literal and then [`validated`](Self::validated), rather than
/// through a many-argument constructor: the fields are all independently meaningful
/// and a caller benefits from naming the ones it sets, while validation stays in
/// one place. A long positional constructor also makes two same-typed arguments
/// transposable without the compiler noticing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    /// The message to create.
    pub id: MessageId,
    /// The conversation it belongs to.
    pub conversation_id: ConversationId,
    /// Who produced it.
    pub role: Role,
    /// Bounded inline content.
    pub content: String,
    /// The content schema version, so a payload can be upcast later.
    pub content_schema_version: i64,
    /// The sensitivity label.
    pub sensitivity: String,
    /// Where the message came from.
    pub source: String,
    /// The instant it was created.
    pub created_at: UtcTimestamp,
}

impl NewMessage {
    /// Validates the message.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when `content` is empty or exceeds
    /// [`MAX_CONTENT_BYTES`], when a label is empty, or when the content schema
    /// version is below one.
    pub fn validated(self) -> Result<Self, RepositoryError> {
        if self.content.is_empty()
            || self.content.len() > MAX_CONTENT_BYTES
            || self.content.contains('\0')
        {
            return Err(RepositoryError::Conflict {
                what: "message_content",
            });
        }
        if self.sensitivity.is_empty() || self.source.is_empty() {
            return Err(RepositoryError::Conflict {
                what: "message_label",
            });
        }
        if self.content_schema_version < 1 {
            return Err(RepositoryError::Conflict {
                what: "content_schema_version",
            });
        }
        Ok(self)
    }
}

/// A loaded message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMessage {
    /// The message identifier.
    pub id: MessageId,
    /// The owning workspace.
    pub workspace_id: WorkspaceId,
    /// The conversation it belongs to.
    pub conversation_id: ConversationId,
    /// Who produced it.
    pub role: Role,
    /// The stored content.
    pub content: String,
    /// Its position in the conversation.
    pub sequence: u64,
    /// The sensitivity label.
    pub sensitivity: String,
    /// The instant it was created.
    pub created_at: UtcTimestamp,
}

/// The durable conversation and message store.
pub trait ConversationRepository: Send + Sync {
    /// Inserts a conversation.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when it already exists.
    fn create_conversation(&self, conversation: NewConversation) -> RepositoryFuture<'_, ()>;

    /// Loads a conversation within `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] when it is absent **or** owned by
    /// another workspace.
    fn load_conversation(
        &self,
        workspace: WorkspaceId,
        conversation: ConversationId,
    ) -> RepositoryFuture<'_, StoredConversation>;

    /// Appends a message at the next position and returns that position.
    ///
    /// The position is assigned server-side so two appends cannot both claim it.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] for an absent or foreign conversation
    /// and [`RepositoryError::Conflict`] when the position was taken concurrently.
    fn append_message(
        &self,
        workspace: WorkspaceId,
        message: NewMessage,
    ) -> RepositoryFuture<'_, u64>;

    /// Loads up to `limit` messages after `after_sequence`, in order.
    ///
    /// A bounded read rather than the whole transcript, because an unbounded one
    /// would let a long conversation be loaded into memory by a single call.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] for an absent or foreign conversation.
    fn load_messages(
        &self,
        workspace: WorkspaceId,
        conversation: ConversationId,
        after_sequence: Option<u64>,
        limit: u32,
    ) -> RepositoryFuture<'_, Vec<StoredMessage>>;

    /// Removes a conversation that has no messages, returning whether one was removed.
    ///
    /// Used to clean up a conversation created for a command that turned out to be a
    /// replay, so a replayed command leaves no trace of the work it did not need.
    ///
    /// The emptiness check is part of the delete rather than a separate read, so a
    /// message appended between a check and a delete cannot be destroyed. A
    /// conversation with any message is left alone and reported as `Ok(false)`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`] for a driver failure.
    fn discard_if_empty(
        &self,
        workspace: WorkspaceId,
        conversation: ConversationId,
    ) -> RepositoryFuture<'_, bool>;
}

#[cfg(test)]
mod tests {
    use super::{MAX_CONTENT_BYTES, MAX_TITLE_BYTES, NewConversation, NewMessage};
    use crate::repository::RepositoryError;
    use jarvis_domain::ids::{ConversationId, MessageId, PrincipalId, WorkspaceId};
    use jarvis_domain::model::stream::Role;
    use jarvis_domain::time::UtcTimestamp;

    fn id(value: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(value)
    }

    fn now() -> UtcTimestamp {
        UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
    }

    fn conversation(
        title: Option<&str>,
        channel: &str,
    ) -> Result<NewConversation, RepositoryError> {
        NewConversation::new(
            ConversationId::from_uuid(id(1)),
            WorkspaceId::from_uuid(id(2)),
            PrincipalId::from_uuid(id(3)),
            title.map(ToOwned::to_owned),
            channel.to_owned(),
            now(),
        )
    }

    fn message(content: &str) -> Result<NewMessage, RepositoryError> {
        NewMessage {
            id: MessageId::from_uuid(id(4)),
            conversation_id: ConversationId::from_uuid(id(1)),
            role: Role::User,
            content: content.to_owned(),
            content_schema_version: 1,
            sensitivity: "internal".to_owned(),
            source: "cli".to_owned(),
            created_at: now(),
        }
        .validated()
    }

    #[test]
    fn a_conversation_title_is_bounded_and_a_channel_is_required() {
        assert!(
            conversation(None, "cli").is_ok(),
            "an untitled conversation is valid"
        );
        assert!(conversation(Some("ok"), "cli").is_ok());
        let over = "a".repeat(MAX_TITLE_BYTES + 1);
        assert!(conversation(Some(&over), "cli").is_err());
        assert!(
            conversation(Some(&"a".repeat(MAX_TITLE_BYTES)), "cli").is_ok(),
            "exactly the bound is inside it",
        );
        assert!(conversation(None, "").is_err(), "a channel is required");
    }

    #[test]
    fn message_content_is_bounded_and_labels_are_required() {
        assert!(message("hello").is_ok());
        assert!(message("").is_err(), "empty content must be refused");
        let over = "a".repeat(MAX_CONTENT_BYTES + 1);
        assert!(message(&over).is_err());
        assert!(
            message(&"a".repeat(MAX_CONTENT_BYTES)).is_ok(),
            "exactly the bound is inside it",
        );

        // A missing sensitivity label would make "unlabelled" indistinguishable
        // from "labelled internal", so it is refused rather than defaulted.
        let no_sensitivity = NewMessage {
            id: MessageId::from_uuid(id(4)),
            conversation_id: ConversationId::from_uuid(id(1)),
            role: Role::User,
            content: "hello".to_owned(),
            content_schema_version: 1,
            sensitivity: String::new(),
            source: "cli".to_owned(),
            created_at: now(),
        }
        .validated();
        assert!(no_sensitivity.is_err());

        let no_schema = NewMessage {
            id: MessageId::from_uuid(id(4)),
            conversation_id: ConversationId::from_uuid(id(1)),
            role: Role::User,
            content: "hello".to_owned(),
            content_schema_version: 0,
            sensitivity: "internal".to_owned(),
            source: "cli".to_owned(),
            created_at: now(),
        }
        .validated();
        assert!(no_schema.is_err(), "a payload needs a schema version");
    }
}
