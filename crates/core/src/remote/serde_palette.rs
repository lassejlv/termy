use crate::TerminalColor;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

pub fn serialize<S: Serializer>(
    colors: &[Option<TerminalColor>; 256],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    colors.as_slice().serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<[Option<TerminalColor>; 256], D::Error> {
    Vec::<Option<TerminalColor>>::deserialize(deserializer)?
        .try_into()
        .map_err(|_| D::Error::custom("invalid terminal palette length"))
}
