use serde::{Serialize, de::DeserializeOwned};

pub trait EventSourced: Sized {
    type Event: Serialize + DeserializeOwned + Clone;
    type Command;
    type Error;

    fn aggregate_type() -> &'static str;
    fn aggregate_id(&self) -> String;
    fn apply(self, event: &Self::Event) -> Self;
    fn handle(&self, command: Self::Command) -> Result<Vec<Self::Event>, Self::Error>;
    fn default_state() -> Self;
}
