use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::time::{Duration, Instant};

pub fn serialize<S: Serializer>(
    deadline: &Option<Instant>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    deadline
        .map(|value| value.saturating_duration_since(Instant::now()))
        .serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Instant>, D::Error> {
    Ok(Option::<Duration>::deserialize(deserializer)?
        .and_then(|delay| Instant::now().checked_add(delay)))
}
