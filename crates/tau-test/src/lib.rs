//! The behaviour lane: a cucumber World and the step definitions the feature
//! tree may use. `bin steps` generates `tests/features/STEPS.md` from
//! `src/steps/` (R-harness-03), so the vocabulary is the code, never a list.

pub mod steps;
pub mod world;
