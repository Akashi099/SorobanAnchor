use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::error::StreamError;
use crate::transaction::{Transaction, TransactionId};

/// Result of polling the underlying stream for the next record.
#[derive(Debug)]
pub enum PollResult {
    /// A record was produced.
    Record(Transaction),
    /// The stream signalled a terminal record carrying the transaction ID.
    Terminal(TransactionId),
    /// The stream reached EOF without a terminal record.
    Eof,
}

/// A traced stream that records the transaction ID of the terminal record.
#[derive(Debug, Default)]
pub struct TracedStream {
    terminal_id: Option<TransactionId>,
}

impl TracedStream {
    pub fn new() -> Self {
        Self { terminal_id: None }
    }

    /// Runs the stream to completion, returning the terminal transaction ID.
    ///
    /// An unexpected EOF (no terminal record observed) is treated as a
    /// failure and surfaces the existing stream error rather than being
    /// reported as a successful completion with an empty transaction ID.
    pub async fn run_traced(
        &mut self,
        stream: Arc<Mutex<dyn Stream>>,
    ) -> Result<TransactionId, StreamError> {
        loop {
            let poll = {
                let mut guard = stream.lock().await;
                guard.poll().await?
            };

            match poll {
                PollResult::Record(_record) => {
                    // Non-terminal records are consumed and the loop continues.
                    continue;
                }
                PollResult::Terminal(id) => {
                    self.terminal_id = Some(id.clone());
                    return Ok(id);
                }
                PollResult::Eof => {
                    // Unexpected EOF: no terminal transaction ID was observed.
                    // Surface the existing stream error instead of reporting a
                    // successful completion with an empty transaction ID.
                    return Err(StreamError::UnexpectedEof);
                }
            }
        }
    }
}

/// Minimal stream abstraction used by [`TracedStream`].
pub trait Stream: Send {
    fn poll(&mut self) -> Result<PollResult, StreamError>;
}

#[cfg(test)]
mod streaming_flow_tests {
    use super::*;

    struct ScriptedStream {
        polls: Vec<PollResult>,
        index: usize,
    }

    impl ScriptedStream {
        fn new(polls: Vec<PollResult>) -> Self {
            Self { polls, index: 0 }
        }
    }

    impl Stream for ScriptedStream {
        fn poll(&mut self) -> Result<PollResult, StreamError> {
            if self.index >= self.polls.len() {
                return Ok(PollResult::Eof);
            }
            let poll = std::mem::replace(&mut self.polls[self.index], PollResult::Eof);
            self.index += 1;
            Ok(poll)
        }
    }

    #[tokio::test]
    async fn unexpected_eof_is_a_failure() {
        let stream: Arc<Mutex<dyn Stream>> = Arc::new(Mutex::new(ScriptedStream::new(vec![
            PollResult::Record(Transaction::default()),
        ])));

        let mut traced = TracedStream::new();
        let result = traced.run_traced(stream).await;

        assert!(
            matches!(result, Err(StreamError::UnexpectedEof)),
            "unexpected EOF must be reported as a failure, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn valid_terminal_record_completes_with_id() {
        let id = TransactionId::from("tx-123");
        let stream: Arc<Mutex<dyn Stream>> = Arc::new(Mutex::new(ScriptedStream::new(vec![
            PollResult::Record(Transaction::default()),
            PollResult::Terminal(id.clone()),
        ])));

        let mut traced = TracedStream::new();
        let result = traced.run_traced(stream).await;

        assert_eq!(result.expect("terminal record should complete"), id);
    }
}
