use std::io::Cursor;
use std::sync::Arc;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::resources::ResourceId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PcmFormat {
    pub channels: u16,
    pub sample_rate: u32,
}

#[derive(Debug)]
pub(super) struct DecodedSound {
    pub format: PcmFormat,
    pub samples: Vec<i16>,
}

pub(super) struct StreamDecoder {
    id: ResourceId,
    encoded_len: usize,
    format_reader: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    format: PcmFormat,
    pending: Vec<i16>,
    pending_offset: usize,
}

impl StreamDecoder {
    pub fn from_bytes(bytes: Arc<[u8]>, id: ResourceId) -> Result<Self, String> {
        let encoded_len = bytes.len();
        let stream = MediaSourceStream::new(
            Box::new(Cursor::new(bytes)),
            MediaSourceStreamOptions::default(),
        );
        let mut hint = Hint::new();
        hint.with_extension("ogg");
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|e| format!("failed to probe {id}: {e}"))?;
        let format_reader = probed.format;
        let track = format_reader
            .default_track()
            .ok_or_else(|| format!("{id} has no default audio track"))?;
        if track.codec_params.codec == CODEC_TYPE_NULL {
            return Err(format!("{id} has no decodable codec"));
        }
        let channels = track
            .codec_params
            .channels
            .ok_or_else(|| format!("{id} has no channel metadata"))?
            .count();
        let channels =
            u16::try_from(channels).map_err(|_| format!("{id} has too many channels"))?;
        if !matches!(channels, 1 | 2) {
            return Err(format!("{id} uses unsupported {channels}-channel audio"));
        }
        let sample_rate = track
            .codec_params
            .sample_rate
            .ok_or_else(|| format!("{id} has no sample-rate metadata"))?;
        let track_id = track.id;
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| format!("failed to create decoder for {id}: {e}"))?;

        Ok(Self {
            id,
            encoded_len,
            format_reader,
            decoder,
            track_id,
            format: PcmFormat {
                channels,
                sample_rate,
            },
            pending: Vec::new(),
            pending_offset: 0,
        })
    }

    pub fn format(&self) -> PcmFormat {
        self.format
    }

    /// Compressed bytes owned by the format reader's cursor until decoder drop.
    pub fn encoded_len(&self) -> usize {
        self.encoded_len
    }

    pub fn read_samples(&mut self, max_samples: usize) -> Result<Vec<i16>, String> {
        let mut output = Vec::with_capacity(max_samples);
        self.drain_pending(&mut output, max_samples);

        while output.len() < max_samples {
            let packet = match self.format_reader.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(e) => {
                    return Err(format!("failed to read {}: {e}", self.id));
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(e) => {
                    return Err(format!("failed to decode {}: {e}", self.id));
                }
            };

            let spec = *decoded.spec();
            if spec.channels.count() != usize::from(self.format.channels)
                || spec.rate != self.format.sample_rate
            {
                return Err(format!("{} changed PCM format mid-stream", self.id));
            }
            let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            samples.copy_interleaved_ref(decoded);
            self.pending.clear();
            self.pending
                .extend(samples.samples().iter().copied().map(vanilla_pcm_i16));
            self.pending_offset = 0;
            self.drain_pending(&mut output, max_samples);
        }

        Ok(output)
    }

    fn drain_pending(&mut self, output: &mut Vec<i16>, max_samples: usize) {
        let wanted = max_samples.saturating_sub(output.len());
        let available = self.pending.len().saturating_sub(self.pending_offset);
        let take = wanted.min(available);
        output.extend_from_slice(&self.pending[self.pending_offset..self.pending_offset + take]);
        self.pending_offset += take;
        if self.pending_offset == self.pending.len() {
            self.pending.clear();
            self.pending_offset = 0;
        }
    }
}

pub(super) fn decode_all(bytes: Arc<[u8]>, id: ResourceId) -> Result<DecodedSound, String> {
    let mut decoder = StreamDecoder::from_bytes(bytes, id)?;
    let format = decoder.format();
    let chunk_samples = usize::try_from(format.sample_rate)
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::from(format.channels));
    let mut samples = Vec::new();
    loop {
        let chunk = decoder.read_samples(chunk_samples)?;
        if chunk.is_empty() {
            break;
        }
        samples.extend(chunk);
    }
    Ok(DecodedSound { format, samples })
}

pub(super) fn vanilla_pcm_i16(sample: f32) -> i16 {
    let value = (sample * 32767.5 - 0.5) as i32;
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_conversion_matches_vanilla_boundaries() {
        assert_eq!(vanilla_pcm_i16(-2.0), i16::MIN);
        assert_eq!(vanilla_pcm_i16(-1.0), i16::MIN);
        assert_eq!(vanilla_pcm_i16(0.0), 0);
        assert_eq!(vanilla_pcm_i16(1.0), i16::MAX);
        assert_eq!(vanilla_pcm_i16(2.0), i16::MAX);
    }
}
