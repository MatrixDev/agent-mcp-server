use serde::{Deserialize, Serialize};

////////////////////////////////////////////////////////////////////////////////
pub const PROTOCOL_VERSION: u32 = 1;

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct AgentPacket<T: AgentPacketContent> {
    pub id: RequestId,
    pub content: T,
}

////////////////////////////////////////////////////////////////////////////////
pub trait AgentPacketContent {}
impl AgentPacketContent for HostRequest {}
impl AgentPacketContent for GuestResponse {}

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub enum HostRequest {
    Initialize(Initialize),
    Ping,
    CommandExec(CommandExec),
    CommandKill(CommandKill),
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub enum GuestResponse {
    Initialized(Initialized),
    GenericResult(GenericResult),
    CommandOutput(CommandOutput),
    CommandFinished(CommandFinished),
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Copy, Clone, Hash)]
#[repr(transparent)]
pub struct RequestId(pub u64);

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Copy, Clone, Hash)]
#[repr(transparent)]
pub struct CommandId(pub u64);

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct Initialize {
    pub version: u32,
    pub session: u64,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct Initialized {
    pub version: u32,
    pub session: u64,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct CommandExec {
    pub id: CommandId,
    pub command: String,
    pub arguments: Vec<String>,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct CommandKill {
    pub id: CommandId,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Copy, Clone, Hash)]
pub enum CommandStream {
    Stdout,
    Stderr,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub id: CommandId,
    pub stream: CommandStream,
    #[serde(with = "serde_bytes")]
    pub line: Vec<u8>,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct GenericResult {
    pub success: bool,
    pub message: String,
}

////////////////////////////////////////////////////////////////////////////////
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct CommandFinished {
    pub id: CommandId,
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

////////////////////////////////////////////////////////////////////////////////
pub fn serialize_cobs(packet: &impl Serialize) -> anyhow::Result<Vec<u8>> {
    Ok(postcard::to_allocvec_cobs(packet)?)
}

////////////////////////////////////////////////////////////////////////////////
pub fn deserialize_cobs<'a, T: Deserialize<'a>>(bytes: &'a mut [u8]) -> anyhow::Result<T> {
    Ok(postcard::from_bytes_cobs(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test() {
        let original = AgentPacket {
            id: RequestId(42),
            content: HostRequest::CommandExec(CommandExec {
                id: CommandId(42),
                command: "do something".to_string(),
                arguments: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            }),
        };

        let mut vec = serialize_cobs(&original).unwrap();
        assert!(!vec.is_empty());
        assert_eq!(vec.last(), Some(&0));

        let parsed = deserialize_cobs(&mut vec).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_output_survives_arbitrary_bytes() {
        let line: Vec<u8> = (0..=255).collect();

        let original = GuestResponse::CommandOutput(CommandOutput {
            id: CommandId(1),
            stream: CommandStream::Stderr,
            line: line.clone(),
        });

        let mut vec = serialize_cobs(&original).unwrap();
        assert_eq!(
            vec.iter().filter(|&&e| e == 0).count(),
            1,
            "only the delimiter may be zero"
        );

        let parsed: GuestResponse = deserialize_cobs(&mut vec).unwrap();
        assert_eq!(original, parsed);

        assert!(vec.len() < line.len() * 2);
    }

    #[test]
    fn test_frames_concatenate_and_split_on_the_delimiter() {
        let first = GuestResponse::GenericResult(GenericResult {
            success: true,
            message: "ok".to_string(),
        });
        let second = GuestResponse::Initialized(Initialized {
            version: PROTOCOL_VERSION,
            session: u64::MAX,
        });

        let mut stream = serialize_cobs(&first).unwrap();
        stream.extend(serialize_cobs(&second).unwrap());

        let mut frames = Vec::new();
        for frame in stream.split_inclusive_mut(|&e| e == 0) {
            frames.push(deserialize_cobs::<GuestResponse>(frame).unwrap());
        }

        assert_eq!(frames, vec![first, second]);
    }
}
