//! Local speech engines. Audio stays in memory and is never written to a recording file.

use sherpa_onnx::{GenerationConfig, OfflineTts, OfflineTtsConfig};
use std::path::{Path, PathBuf};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub fn model_root() -> PathBuf {
    std::env::var_os("OMNICORE_VOICE_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/voice-models"))
}

pub fn check_models(voice: &str) -> Result<(), String> {
    if !matches!(voice, "en_US-lessac-medium" | "en_US-amy-low") {
        return Err("Unsupported speaking voice".into());
    }
    let root = model_root();
    for path in [
        root.join("ggml-base.en.bin"),
        root.join(format!("vits-piper-{voice}"))
            .join(format!("{voice}.onnx")),
    ] {
        if !path.is_file() {
            return Err(format!("Voice model unavailable: {}", path.display()));
        }
    }
    Ok(())
}

pub fn load_stt(model: &Path) -> Result<WhisperContext, String> {
    // whisper.cpp otherwise writes token-by-token transcripts to stderr.
    // With whisper-rs default features, these hooks discard its native logs.
    whisper_rs::install_logging_hooks();
    WhisperContext::new_with_params(model, WhisperContextParameters::default())
        .map_err(|e| format!("Speech model unavailable: {e}"))
}

pub fn transcribe(pcm: &[i16], context: &WhisperContext) -> Result<String, String> {
    if pcm.len() < 16_000 / 3 || pcm.len() > 16_000 * 12 {
        return Err("Speech segment length is outside supported limits".into());
    }
    let mut state = context
        .create_state()
        .map_err(|e| format!("Speech engine failed: {e}"))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("en"));
    params.set_no_context(true);
    params.set_n_threads(3);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    let samples: Vec<f32> = pcm
        .iter()
        .map(|&sample| f32::from(sample) / 32768.0)
        .collect();
    state
        .full(params, &samples)
        .map_err(|e| format!("Speech recognition failed: {e}"))?;
    let mut result = String::new();
    for segment in state.as_iter() {
        let text = segment
            .to_str_lossy()
            .map_err(|e| format!("Speech result unreadable: {e}"))?;
        result.push_str(&text);
    }
    Ok(result.trim().to_owned())
}

pub fn synthesize(text: &str, voice: &str, root: &Path) -> Result<Vec<u8>, String> {
    if !matches!(voice, "en_US-lessac-medium" | "en_US-amy-low") {
        return Err("Unsupported speaking voice".into());
    }
    if text.is_empty() || text.len() > 1_500 || text.contains('\0') {
        return Err("Invalid speech text".into());
    }
    let directory = root.join(format!("vits-piper-{voice}"));
    let mut config = OfflineTtsConfig::default();
    config.model.vits.model = Some(
        directory
            .join(format!("{voice}.onnx"))
            .to_string_lossy()
            .into(),
    );
    config.model.vits.tokens = Some(directory.join("tokens.txt").to_string_lossy().into());
    config.model.vits.data_dir = Some(directory.join("espeak-ng-data").to_string_lossy().into());
    config.model.num_threads = 2;
    let tts = OfflineTts::create(&config).ok_or("Speaking voice could not load")?;
    let generated = tts
        .generate_with_config(
            text,
            &GenerationConfig::default(),
            None::<fn(&[f32], f32) -> bool>,
        )
        .ok_or("Speech synthesis failed")?;
    let rate = u32::try_from(generated.sample_rate()).map_err(|_| "Invalid voice sample rate")?;
    wav_mono(generated.samples(), rate)
}

fn wav_mono(samples: &[f32], rate: u32) -> Result<Vec<u8>, String> {
    if rate == 0 || samples.len() > 48_000 * 45 {
        return Err("Generated speech exceeds audio limit".into());
    }
    let data_len = u32::try_from(samples.len() * 2).map_err(|_| "Speech too long")?;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_valid_and_bounded() {
        let wave = wav_mono(&[0.0, 0.5, -0.5], 22_050).unwrap();
        assert_eq!(&wave[..4], b"RIFF");
        assert_eq!(&wave[8..12], b"WAVE");
        assert_eq!(wave.len(), 50);
        assert!(wav_mono(&[], 0).is_err());
    }

    #[test]
    #[ignore = "requires the installed external voice models"]
    fn local_speech_engines_roundtrip() {
        let root = model_root();
        check_models("en_US-lessac-medium").unwrap();
        let tts_started = std::time::Instant::now();
        let wav = synthesize(
            "Hello OmniCore. Voice chat is working.",
            "en_US-lessac-medium",
            &root,
        )
        .unwrap();
        let tts_elapsed = tts_started.elapsed();
        assert_eq!(&wav[8..12], b"WAVE");
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            songbird::input::Input::from(wav.clone())
                .make_playable_async(
                    songbird::input::codecs::get_codec_registry(),
                    songbird::input::codecs::get_probe(),
                )
                .await
                .expect("Discord playback must decode the in-memory WAV");
        });
        let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
        let raw: Vec<i16> = wav[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let count = raw.len() * 16_000 / rate as usize;
        let pcm: Vec<i16> = (0..count)
            .map(|i| raw[i * rate as usize / 16_000])
            .collect();
        let model = load_stt(&root.join("ggml-base.en.bin")).unwrap();
        let stt_started = std::time::Instant::now();
        let transcript = transcribe(&pcm, &model).unwrap();
        let stt_elapsed = stt_started.elapsed();
        eprintln!(
            "Local speech benchmark: TTS {} ms, STT {} ms",
            tts_elapsed.as_millis(),
            stt_elapsed.as_millis()
        );
        assert!(
            transcript.to_ascii_lowercase().contains("hello"),
            "transcription did not match test audio: {transcript}"
        );
    }
}
