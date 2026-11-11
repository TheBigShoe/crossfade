//! The job model: plain data describing what to convert, serialisable so a
//! front end can save, show or replay it.

use crate::filter::{Channels, FilterError, FilterSpec, check_rate, dither_name};
use crossfade_core::{Error, SampleFormat};
use crossfade_dsp::dither::Dither;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub version: u32,
    pub inputs: Vec<JobInput>,
    pub outputs: Vec<JobOutput>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobInput {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobOutput {
    pub path: String,
    pub audio: AudioSettings,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AudioSettings {
    #[serde(default, skip_serializing_if = "Option::is_none", with = "format_name")]
    pub sample_format: Option<SampleFormat>,
    /// `"copy"`, `"pcm"`, `"flac"`; `None` is the container's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// FLAC only, 0 to 8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression_level: Option<u8>,
    /// Output rate in Hz; `None` keeps the rate the filters leave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// Output channels; `None` keeps the layout the filters leave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<Channels>,
    /// How processed audio becomes integer samples; only used when filters or a rate or
    /// channel change ran.
    #[serde(default, skip_serializing_if = "is_tpdf", with = "dither_name")]
    pub dither: Dither,
    /// Run in order, before the conversions `sample_rate` and `channels` ask for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<FilterSpec>,
}

fn is_tpdf(d: &Dither) -> bool {
    *d == Dither::Tpdf
}

impl AudioSettings {
    /// The rules on values a job file could break: rate, channels and every filter.
    pub fn check(&self) -> Result<(), FilterError> {
        if let Some(rate) = self.sample_rate {
            check_rate(rate).map_err(|m| FilterError::at(&rate.to_string(), m))?;
        }
        for f in &self.filters {
            f.check()?;
        }
        Ok(())
    }
}

/// `SampleFormat` stays serde-free in core; the job spells it by name.
mod format_name {
    use super::SampleFormat;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(
        value: &Option<SampleFormat>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(f) => s.serialize_str(f.name()),
            None => s.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<SampleFormat>, D::Error> {
        let Some(name) = Option::<String>::deserialize(d)? else {
            return Ok(None);
        };
        SampleFormat::from_name(&name)
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown sample format {name:?}")))
    }
}

impl Job {
    pub const VERSION: u32 = 1;

    pub fn to_json_pretty(&self) -> String {
        // Plain data with string keys cannot fail to serialise.
        serde_json::to_string_pretty(self).expect("job serialises")
    }

    pub fn from_json(json: &str) -> Result<Job, Error> {
        let job: Job = serde_json::from_str(json).map_err(|e| Error::Malformed {
            what: format!("job: {e}"),
        })?;
        for output in &job.outputs {
            output.audio.check().map_err(|e| Error::Malformed {
                what: format!("job: {}: {e}", output.path),
            })?;
        }
        if job.version != Self::VERSION {
            return Err(Error::Unsupported {
                what: format!(
                    "job version {} (this build reads {})",
                    job.version,
                    Self::VERSION
                ),
            });
        }
        Ok(job)
    }
}
