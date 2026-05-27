pub mod cargo_target;
pub mod json_io;
pub mod subprocess;
pub mod walk;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stack {
    Rust,
    Csharp,
    Both,
    None,
}
