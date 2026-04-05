use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct HolotapeGameState {
    pub score: u32,
    pub level: u32,
    // Note: Expand this struct based on the exact fields the minigame requires.
}

/// Parses the custom binary `.pip` format into a JSON string.
pub fn parse_pip_to_json(pip_data: &[u8]) -> Result<String> {
    if pip_data.len() < 8 {
        // Return default state if data is missing or too short
        let default_state = HolotapeGameState::default();
        return Ok(serde_json::to_string_pretty(&default_state)?);
    }

    // TODO: Replace these offsets with the actual Fallout 76 .pip binary schema.
    // Example: Reading little-endian u32s from the binary buffer.
    let score = u32::from_le_bytes(pip_data[0..4].try_into()?);
    let level = u32::from_le_bytes(pip_data[4..8].try_into()?);

    let state = HolotapeGameState { score, level };

    Ok(serde_json::to_string_pretty(&state)?)
}

/// Serializes the JSON string back into the `.pip` binary format.
pub fn parse_json_to_pip(json_data: &str) -> Result<Vec<u8>> {
    let state: HolotapeGameState = serde_json::from_str(json_data)?;

    let mut pip_data = Vec::with_capacity(8);

    // TODO: Replace with the actual binary packing sequence expected by the SWF.
    pip_data.extend_from_slice(&state.score.to_le_bytes());
    pip_data.extend_from_slice(&state.level.to_le_bytes());

    Ok(pip_data)
}
