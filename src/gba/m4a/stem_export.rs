//! Multi-Track 48 kHz WAV Stem Exporter
//!
//! Renders each sequence track / instrument into isolated, time-aligned
//! 48 kHz stereo WAV stems alongside a master mix file.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use super::sampler::{HdM4aSampler, M4aInterpolation};
use super::song::{M4aEvent, M4aSequencer, SongHeader};
use super::voice::ToneData;

/// An exported audio stem containing time-aligned stereo samples
#[derive(Debug, Clone)]
pub struct StemTrack {
    pub name: String,
    pub channel_index: usize,
    /// Interleaved stereo samples at target sample rate (L, R, L, R, ...)
    pub samples: Vec<f32>,
}

/// Render an M4A song into isolated track stems and a master mix
pub fn render_song_to_stems(
    rom: &[u8],
    song_header: &SongHeader,
    duration_secs: f32,
    sample_rate: u32,
    interpolation: M4aInterpolation,
) -> Vec<StemTrack> {
    let num_tracks = song_header.track_count as usize;
    let total_samples = (duration_secs * (sample_rate as f32)) as usize;

    let mut master_samples = Vec::with_capacity(total_samples * 2);
    let mut track_samples = Vec::with_capacity(num_tracks);
    for _ in 0..num_tracks {
        track_samples.push(Vec::with_capacity(total_samples * 2));
    }

    let mut sequencer = M4aSequencer::new(song_header.clone(), rom);
    let mut master_sampler = HdM4aSampler::new(64, sample_rate);
    master_sampler.interpolation = interpolation;

    // Per-track samplers for isolated stems
    let mut track_samplers = Vec::with_capacity(num_tracks);
    for _ in 0..num_tracks {
        let mut s = HdM4aSampler::new(32, sample_rate);
        s.interpolation = interpolation;
        s.reverb_enabled = false; // stems dry by default for mixing
        track_samplers.push(s);
    }

    // Cached tone table
    let tone_ptr = song_header.tone_ptr;

    for _ in 0..total_samples {
        // Step sequencer and dispatch events
        let events = sequencer.step_sample(rom, sample_rate);
        for event in events {
            match event {
                M4aEvent::NoteOn { channel, key, velocity, .. } => {
                    let voice_idx = if channel < sequencer.tracks.len() {
                        sequencer.tracks[channel].voice
                    } else {
                        0
                    };
                    let (vol, pan, bend, tune) = if channel < sequencer.tracks.len() {
                        let tr = &sequencer.tracks[channel];
                        (tr.volume, tr.pan, tr.pitch_bend, tr.tune)
                    } else {
                        (100, 64, 0, 0)
                    };

                    if let Some(base_tone) = ToneData::parse_from_rom(rom, tone_ptr, voice_idx) {
                        if let Some((tone, wave)) = base_tone.resolve_for_key(rom, key) {
                            master_sampler.note_on(channel, key, velocity, &tone, wave.clone(), pan, vol, bend, tune);
                            if channel < track_samplers.len() {
                                track_samplers[channel].note_on(channel, key, velocity, &tone, wave, pan, vol, bend, tune);
                            }
                        }
                    }
                }
                M4aEvent::NoteOff { channel, key } => {
                    master_sampler.note_off(channel, key);
                    if channel < track_samplers.len() {
                        track_samplers[channel].note_off(channel, key);
                    }
                }
                M4aEvent::VolumeChange { channel, volume } => {
                    master_sampler.set_volume(channel, volume);
                    if channel < track_samplers.len() {
                        track_samplers[channel].set_volume(channel, volume);
                    }
                }
                M4aEvent::PanChange { channel, pan } => {
                    master_sampler.set_pan(channel, pan);
                    if channel < track_samplers.len() {
                        track_samplers[channel].set_pan(channel, pan);
                    }
                }
                _ => {}
            }
        }

        // Render master sample
        let (ml, mr) = master_sampler.render_sample();
        master_samples.push(ml);
        master_samples.push(mr);

        // Render each track sample
        for (ch, sampler) in track_samplers.iter_mut().enumerate() {
            let (sl, sr) = sampler.render_sample();
            track_samples[ch].push(sl);
            track_samples[ch].push(sr);
        }
    }

    let mut result = Vec::with_capacity(num_tracks + 1);

    // 0. Master stem
    result.push(StemTrack {
        name: "Master Mix".to_string(),
        channel_index: 0,
        samples: master_samples,
    });

    // 1..=N. Track stems
    for (i, samples) in track_samples.into_iter().enumerate() {
        result.push(StemTrack {
            name: format!("Stem {:02} Track {}", i + 1, i + 1),
            channel_index: i + 1,
            samples,
        });
    }

    result
}

/// Write 16-bit PCM stereo WAV file to disk
pub fn write_wav_file(path: &Path, samples: &[f32], sample_rate: u32) -> io::Result<()> {
    let mut file = File::create(path)?;

    let num_channels: u16 = 2;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * (num_channels as u32) * ((bits_per_sample / 8) as u32);
    let block_align = num_channels * (bits_per_sample / 8);

    let num_pcm_frames = samples.len() / (num_channels as usize);
    let data_bytes = (num_pcm_frames * (block_align as usize)) as u32;
    let riff_size = 36 + data_bytes;

    // RIFF Chunk
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVE")?;

    // fmt  Sub-chunk
    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?; // SubChunk1Size (16 for PCM)
    file.write_all(&1u16.to_le_bytes())?;  // AudioFormat (1 for PCM)
    file.write_all(&num_channels.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&bits_per_sample.to_le_bytes())?;

    // data Sub-chunk
    file.write_all(b"data")?;
    file.write_all(&data_bytes.to_le_bytes())?;

    // Convert f32 (-1.0..1.0) to i16 PCM
    let mut pcm_buf = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let sample_i16 = (clamped * 32767.0) as i16;
        pcm_buf.extend_from_slice(&sample_i16.to_le_bytes());
    }
    file.write_all(&pcm_buf)?;

    Ok(())
}
