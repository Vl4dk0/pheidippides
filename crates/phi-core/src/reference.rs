use phi_proto::{MessageId, ValidationError};
use std::{fmt, num::NonZeroUsize, str::FromStr};

/// Thread positions are a client convenience; signed IDs remain the wire identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRef {
    Id(MessageId),
    InThread {
        thread: MessageId,
        number: NonZeroUsize,
    },
}
impl FromStr for MessageRef {
    type Err = ValidationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some((thread, number)) = value.split_once('#') {
            let number = number.parse::<NonZeroUsize>().map_err(|_| {
                ValidationError::new(
                    "message reference",
                    "expected <thread-id>#<positive message number>",
                )
            })?;
            Ok(Self::InThread {
                thread: thread.parse()?,
                number,
            })
        } else {
            Ok(Self::Id(value.parse()?))
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageReference {
    pub thread: MessageId,
    pub number: usize,
    pub id: MessageId,
}
impl fmt::Display for MessageReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.thread, self.number)
    }
}
