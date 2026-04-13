//! Application state and server event types

use crate::log_buffer::LogBuffer;
use crate::link_preview_queue::LinkPreviewQueue;
use atomic_core::{AtomicCore, DatabaseManager};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Shared application state for all route handlers
pub struct AppState {
    pub manager: Arc<DatabaseManager>,
    pub event_tx: broadcast::Sender<ServerEvent>,
    /// Public URL for OAuth discovery (set via --public-url CLI flag)
    pub public_url: Option<String>,
    /// In-memory ring buffer for recent log lines (for user export)
    pub log_buffer: LogBuffer,
    /// Async queue for link screenshot generation jobs.
    pub link_preview_queue: LinkPreviewQueue,
}

impl AppState {
    /// Resolve which database core to use for a request.
    /// Checks X-Atomic-Database header, then ?db= query param, then falls back to active.
    pub fn resolve_core(&self, req: &actix_web::HttpRequest) -> Result<AtomicCore, atomic_core::AtomicCoreError> {
        // Check X-Atomic-Database header
        if let Some(db_id) = req.headers().get("X-Atomic-Database")
            .and_then(|v| v.to_str().ok())
        {
            return self.manager.get_core(db_id);
        }

        // Check ?db= query parameter
        if let Some(db_id) = req.query_string()
            .split('&')
            .find_map(|pair| {
                let mut parts = pair.splitn(2, '=');
                if parts.next()? == "db" { parts.next() } else { None }
            })
        {
            return self.manager.get_core(db_id);
        }

        // Default to active database
        self.manager.active_core()
    }
}

/// Events broadcast to WebSocket clients
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ServerEvent {
    // Embedding pipeline events
    EmbeddingStarted {
        atom_id: String,
    },
    EmbeddingComplete {
        atom_id: String,
    },
    EmbeddingFailed {
        atom_id: String,
        error: String,
    },
    TaggingComplete {
        atom_id: String,
        tags_extracted: Vec<String>,
        new_tags_created: Vec<String>,
    },
    TaggingFailed {
        atom_id: String,
        error: String,
    },
    TaggingSkipped {
        atom_id: String,
    },
    BatchProgress {
        batch_id: String,
        phase: String,
        completed: usize,
        total: usize,
    },

    // Atom lifecycle events
    AtomCreated {
        atom: atomic_core::AtomWithTags,
    },

    // Import progress events
    ImportProgress {
        current: i32,
        total: i32,
        current_file: String,
        status: String,
    },

    // Ingestion pipeline events
    IngestionFetchStarted {
        url: String,
        request_id: String,
    },
    IngestionFetchComplete {
        url: String,
        request_id: String,
        content_length: usize,
    },
    IngestionFetchFailed {
        url: String,
        request_id: String,
        error: String,
    },
    IngestionSkipped {
        url: String,
        request_id: String,
        reason: String,
    },
    IngestionComplete {
        request_id: String,
        atom_id: String,
        url: String,
        title: String,
    },
    IngestionFailed {
        request_id: String,
        url: String,
        error: String,
    },
    FeedPollComplete {
        feed_id: String,
        new_items: i32,
        skipped: i32,
        errors: i32,
    },
    FeedPollFailed {
        feed_id: String,
        error: String,
    },

    // Chat streaming events
    ChatStreamDelta {
        conversation_id: String,
        content: String,
    },
    ChatToolStart {
        conversation_id: String,
        tool_call_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
    },
    ChatToolComplete {
        conversation_id: String,
        tool_call_id: String,
        results_count: i32,
    },
    ChatComplete {
        conversation_id: String,
        message: atomic_core::ChatMessageWithContext,
    },
    ChatCanvasAction {
        conversation_id: String,
        action: String,
        params: serde_json::Value,
    },
    ChatError {
        conversation_id: String,
        error: String,
    },

    // Scheduled task events
    BriefingReady {
        db_id: String,
        briefing_id: String,
    },
}

impl From<atomic_core::EmbeddingEvent> for ServerEvent {
    fn from(event: atomic_core::EmbeddingEvent) -> Self {
        match event {
            atomic_core::EmbeddingEvent::Started { atom_id } => {
                ServerEvent::EmbeddingStarted { atom_id }
            }
            atomic_core::EmbeddingEvent::EmbeddingComplete { atom_id } => {
                ServerEvent::EmbeddingComplete { atom_id }
            }
            atomic_core::EmbeddingEvent::EmbeddingFailed { atom_id, error } => {
                ServerEvent::EmbeddingFailed { atom_id, error }
            }
            atomic_core::EmbeddingEvent::TaggingComplete {
                atom_id,
                tags_extracted,
                new_tags_created,
            } => ServerEvent::TaggingComplete {
                atom_id,
                tags_extracted,
                new_tags_created,
            },
            atomic_core::EmbeddingEvent::TaggingFailed { atom_id, ref error } => {
                tracing::warn!(atom_id, error = %error, "Tagging failed");
                ServerEvent::TaggingFailed { atom_id, error: error.clone() }
            }
            atomic_core::EmbeddingEvent::TaggingSkipped { atom_id } => {
                ServerEvent::TaggingSkipped { atom_id }
            }
            atomic_core::EmbeddingEvent::BatchProgress { batch_id, phase, completed, total } => {
                ServerEvent::BatchProgress { batch_id, phase, completed, total }
            }
        }
    }
}

impl From<atomic_core::IngestionEvent> for ServerEvent {
    fn from(event: atomic_core::IngestionEvent) -> Self {
        match event {
            atomic_core::IngestionEvent::FetchStarted { url, request_id } => {
                ServerEvent::IngestionFetchStarted { url, request_id }
            }
            atomic_core::IngestionEvent::FetchComplete { url, request_id, content_length } => {
                ServerEvent::IngestionFetchComplete { url, request_id, content_length }
            }
            atomic_core::IngestionEvent::FetchFailed { url, request_id, error } => {
                ServerEvent::IngestionFetchFailed { url, request_id, error }
            }
            atomic_core::IngestionEvent::Skipped { url, request_id, reason } => {
                ServerEvent::IngestionSkipped { url, request_id, reason }
            }
            atomic_core::IngestionEvent::IngestionComplete { request_id, atom_id, url, title } => {
                ServerEvent::IngestionComplete { request_id, atom_id, url, title }
            }
            atomic_core::IngestionEvent::IngestionFailed { request_id, url, error } => {
                ServerEvent::IngestionFailed { request_id, url, error }
            }
            atomic_core::IngestionEvent::FeedPollComplete { feed_id, new_items, skipped, errors } => {
                ServerEvent::FeedPollComplete { feed_id, new_items, skipped, errors }
            }
            atomic_core::IngestionEvent::FeedPollFailed { feed_id, error } => {
                ServerEvent::FeedPollFailed { feed_id, error }
            }
        }
    }
}

impl From<atomic_core::ChatEvent> for ServerEvent {
    fn from(event: atomic_core::ChatEvent) -> Self {
        match event {
            atomic_core::ChatEvent::StreamDelta {
                conversation_id,
                content,
            } => ServerEvent::ChatStreamDelta {
                conversation_id,
                content,
            },
            atomic_core::ChatEvent::ToolStart {
                conversation_id,
                tool_call_id,
                tool_name,
                tool_input,
            } => ServerEvent::ChatToolStart {
                conversation_id,
                tool_call_id,
                tool_name,
                tool_input,
            },
            atomic_core::ChatEvent::ToolComplete {
                conversation_id,
                tool_call_id,
                results_count,
            } => ServerEvent::ChatToolComplete {
                conversation_id,
                tool_call_id,
                results_count,
            },
            atomic_core::ChatEvent::Complete {
                conversation_id,
                message,
            } => ServerEvent::ChatComplete {
                conversation_id,
                message,
            },
            atomic_core::ChatEvent::CanvasAction {
                conversation_id,
                action,
                params,
            } => ServerEvent::ChatCanvasAction {
                conversation_id,
                action,
                params,
            },
            atomic_core::ChatEvent::Error {
                conversation_id,
                error,
            } => ServerEvent::ChatError {
                conversation_id,
                error,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedding_started_conversion() {
        let event = atomic_core::EmbeddingEvent::Started {
            atom_id: "a1".into(),
        };
        let server_event = ServerEvent::from(event);
        match server_event {
            ServerEvent::EmbeddingStarted { atom_id } => assert_eq!(atom_id, "a1"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_embedding_complete_conversion() {
        let event = atomic_core::EmbeddingEvent::EmbeddingComplete {
            atom_id: "a2".into(),
        };
        match ServerEvent::from(event) {
            ServerEvent::EmbeddingComplete { atom_id } => assert_eq!(atom_id, "a2"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_embedding_failed_conversion() {
        let event = atomic_core::EmbeddingEvent::EmbeddingFailed {
            atom_id: "a3".into(),
            error: "timeout".into(),
        };
        match ServerEvent::from(event) {
            ServerEvent::EmbeddingFailed { atom_id, error } => {
                assert_eq!(atom_id, "a3");
                assert_eq!(error, "timeout");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_tagging_complete_conversion() {
        let event = atomic_core::EmbeddingEvent::TaggingComplete {
            atom_id: "a4".into(),
            tags_extracted: vec!["t1".into()],
            new_tags_created: vec!["t2".into()],
        };
        match ServerEvent::from(event) {
            ServerEvent::TaggingComplete {
                atom_id,
                tags_extracted,
                new_tags_created,
            } => {
                assert_eq!(atom_id, "a4");
                assert_eq!(tags_extracted, vec!["t1"]);
                assert_eq!(new_tags_created, vec!["t2"]);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_chat_stream_delta_conversion() {
        let event = atomic_core::ChatEvent::StreamDelta {
            conversation_id: "c1".into(),
            content: "hello".into(),
        };
        match ServerEvent::from(event) {
            ServerEvent::ChatStreamDelta {
                conversation_id,
                content,
            } => {
                assert_eq!(conversation_id, "c1");
                assert_eq!(content, "hello");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_chat_tool_start_conversion() {
        let event = atomic_core::ChatEvent::ToolStart {
            conversation_id: "c2".into(),
            tool_call_id: "tc1".into(),
            tool_name: "search".into(),
            tool_input: serde_json::json!({"query": "test"}),
        };
        match ServerEvent::from(event) {
            ServerEvent::ChatToolStart {
                conversation_id,
                tool_name,
                tool_input,
                ..
            } => {
                assert_eq!(conversation_id, "c2");
                assert_eq!(tool_name, "search");
                assert_eq!(tool_input["query"], "test");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_chat_error_conversion() {
        let event = atomic_core::ChatEvent::Error {
            conversation_id: "c3".into(),
            error: "api failed".into(),
        };
        match ServerEvent::from(event) {
            ServerEvent::ChatError {
                conversation_id,
                error,
            } => {
                assert_eq!(conversation_id, "c3");
                assert_eq!(error, "api failed");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_server_event_serializes_with_type_tag() {
        let event = ServerEvent::EmbeddingComplete {
            atom_id: "a1".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "EmbeddingComplete");
        assert_eq!(json["atom_id"], "a1");
    }

    #[test]
    fn test_event_broadcast_delivery() {
        let (tx, mut rx) = broadcast::channel::<ServerEvent>(16);
        let event = ServerEvent::EmbeddingStarted {
            atom_id: "a1".into(),
        };
        tx.send(event).unwrap();

        let received = rx.try_recv().unwrap();
        match received {
            ServerEvent::EmbeddingStarted { atom_id } => assert_eq!(atom_id, "a1"),
            _ => panic!("Wrong variant"),
        }
    }
}
