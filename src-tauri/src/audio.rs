//! Captura de microfone e medição de nível.
//!
//! ## Por que existe uma thread dedicada
//!
//! `cpal::Stream` não é `Send` nem `Sync` — ele guarda ponteiros crus do driver
//! de áudio do sistema. Como o `State` do Tauri exige `Send + Sync`, o stream
//! não pode morar na struct compartilhada: uma thread própria o cria, toca e
//! destrói, sem nunca deixá-lo atravessar fronteira de thread.
//!
//! O que atravessa é só o `Arc<Mutex<Shared>>` com as amostras — esse sim é
//! seguro de compartilhar.
//!
//! ## Por que a conversão acontece no callback
//!
//! A redução para 16 kHz / mono / 16-bit roda **dentro** do callback do
//! dispositivo, e não num passe posterior. Assim, quando o usuário solta o
//! atalho, o buffer já está no formato de envio e o upload começa na hora, sem
//! um passe de conversão sobre vários megabytes.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use parking_lot::Mutex;

/// Formato exigido pela speech-api.
pub const TARGET_RATE: u32 = 16_000;
pub const TARGET_CHANNELS: u16 = 1;

/// Amostras por janela de nível. Em 16 kHz, 10 ms.
const LEVEL_WINDOW: usize = (TARGET_RATE as usize) / 100;

/// Acima disto consideramos que houve som de fala. É conservador de propósito:
/// quem decide de verdade é o modelo de transcrição; isto só evita anunciar
/// "nenhuma fala" quando houve sinal claro.
const SPEECH_THRESHOLD: f32 = 0.06;

#[derive(Default)]
struct Shared {
    samples: Vec<i16>,
    /// Um valor por janela de 10 ms. O HUD desenha a cauda disto.
    envelope: Vec<f32>,
    peak: f32,
    speech_detected: bool,
    /// Até onde `drain_new_samples` já entregou. O buffer não é consumido —
    /// só marcamos a posição, porque o `finish()` precisa dele inteiro.
    streamed_upto: usize,
}

/// Descrição da fonte, devolvida pela thread depois de abrir o dispositivo.
struct SourceInfo {
    device_name: String,
    source_rate: u32,
    source_channels: u16,
}

pub struct Capture {
    shared: Arc<Mutex<Shared>>,
    stop: Option<Sender<()>>,
    worker: Option<JoinHandle<()>>,
    info: SourceInfo,
}

pub struct Recording {
    pub samples: Vec<i16>,
    pub peak: f32,
    pub speech_detected: bool,
    pub device_name: String,
}

impl Recording {
    pub fn duration_seconds(&self) -> f32 {
        self.samples.len() as f32 / TARGET_RATE as f32
    }

    /// Empacota como WAV PCM 16 kHz mono, que é o que `/v1/audio/transcriptions`
    /// aceita.
    pub fn to_wav(&self) -> Vec<u8> {
        let data_len = (self.samples.len() * 2) as u32;
        let block_align: u16 = TARGET_CHANNELS * 2;
        let byte_rate = TARGET_RATE * block_align as u32;

        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&TARGET_CHANNELS.to_le_bytes());
        out.extend_from_slice(&TARGET_RATE.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for sample in &self.samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub fn list_input_devices() -> Result<Vec<InputDevice>> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|device| device.name().ok());

    let mut devices = Vec::new();
    for device in host.input_devices().context("enumerar entradas")? {
        let Ok(name) = device.name() else { continue };
        devices.push(InputDevice {
            is_default: Some(&name) == default_name.as_ref(),
            id: name.clone(),
            name,
        });
    }
    Ok(devices)
}

impl Capture {
    /// Abre o microfone e começa a gravar imediatamente.
    ///
    /// Só retorna depois que o dispositivo abriu de fato, para o chamador saber
    /// que o áudio está sendo capturado antes de mostrar o HUD.
    pub fn start(preferred_device: Option<&str>) -> Result<Self> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let sink = shared.clone();
        let preferred = preferred_device.map(str::to_owned);

        let (ready_tx, ready_rx) = mpsc::channel::<Result<SourceInfo, String>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        let worker = std::thread::Builder::new()
            .name("vox-audio".into())
            .spawn(move || run_capture(sink, preferred, ready_tx, stop_rx))
            .context("subir a thread de áudio")?;

        let info = ready_rx
            .recv()
            .context("a thread de áudio terminou sem responder")?
            .map_err(|message| anyhow!(message))?;

        tracing::info!(
            device = %info.device_name,
            source_rate = info.source_rate,
            source_channels = info.source_channels,
            target_rate = TARGET_RATE,
            "captura iniciada"
        );

        Ok(Self {
            shared,
            stop: Some(stop_tx),
            worker: Some(worker),
            info,
        })
    }

    /// Envelope reduzido a `bar_count` barras, normalizado em 0..1.
    ///
    /// Devolve sempre `bar_count` valores, mesmo no silêncio inicial, para o
    /// desenho não mudar de largura no meio da gravação.
    pub fn levels(&self, bar_count: usize) -> Vec<f32> {
        if bar_count == 0 {
            return Vec::new();
        }

        let shared = self.shared.lock();
        if shared.envelope.is_empty() {
            return vec![0.0; bar_count];
        }

        // Só a cauda interessa: é a onda andando da direita para a esquerda.
        let window = bar_count.min(shared.envelope.len());
        let tail = &shared.envelope[shared.envelope.len() - window..];

        let mut bars = vec![0.0f32; bar_count - window];
        bars.extend_from_slice(tail);

        // Normaliza pelo pico da própria janela, com piso para o silêncio não
        // virar ruído amplificado.
        let ceiling = bars.iter().copied().fold(0.0f32, f32::max).max(0.02);
        for bar in &mut bars {
            *bar = (*bar / ceiling).clamp(0.0, 1.0);
        }
        bars
    }

    /// Amostras capturadas desde a última chamada.
    ///
    /// Existe para o streaming de transcrição poder empurrar áudio enquanto a
    /// pessoa ainda fala. Devolve só o trecho novo e guarda até onde já leu, em
    /// vez de copiar o buffer inteiro a cada vez — numa gravação de um minuto
    /// isso seria quase um megabyte copiado a cada 250 ms.
    ///
    /// O buffer completo continua intacto para o `finish()`: o caminho em lote
    /// ainda precisa dele inteiro.
    pub fn drain_new_samples(&self) -> Vec<i16> {
        let mut shared = self.shared.lock();
        let ja_lido = shared.streamed_upto;
        if ja_lido >= shared.samples.len() {
            return Vec::new();
        }
        let novo = shared.samples[ja_lido..].to_vec();
        shared.streamed_upto = shared.samples.len();
        novo
    }

    pub fn source_description(&self) -> String {
        format!(
            "{} ({} Hz, {} ch)",
            self.info.device_name, self.info.source_rate, self.info.source_channels
        )
    }

    /// Encerra a captura e devolve o áudio já no formato de envio.
    pub fn finish(mut self) -> Recording {
        self.shutdown();
        let shared = self.shared.lock();
        Recording {
            samples: shared.samples.clone(),
            peak: shared.peak,
            speech_detected: shared.speech_detected,
            device_name: self.info.device_name.clone(),
        }
    }

    fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Capture {
    /// Um ditado cancelado descarta o `Capture` sem chamar `finish`; sem isto a
    /// thread ficaria viva segurando o microfone aberto.
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Corpo da thread de áudio. O `Stream` nasce, vive e morre aqui dentro.
fn run_capture(
    sink: Arc<Mutex<Shared>>,
    preferred: Option<String>,
    ready: Sender<Result<SourceInfo, String>>,
    stop: Receiver<()>,
) {
    let stream = match build_stream(sink, preferred.as_deref()) {
        Ok((stream, info)) => {
            if ready.send(Ok(info)).is_err() {
                return; // ninguém mais espera por isto
            }
            stream
        }
        Err(err) => {
            let _ = ready.send(Err(err.to_string()));
            return;
        }
    };

    if let Err(err) = stream.play() {
        tracing::error!(?err, "não foi possível iniciar o stream");
        return;
    }

    // Fica parada até mandarem parar. Se o outro lado sumir, o `recv` devolve
    // erro e a thread encerra igual — sem microfone preso.
    let _ = stop.recv();
    drop(stream);
}

fn build_stream(
    sink: Arc<Mutex<Shared>>,
    preferred: Option<&str>,
) -> Result<(cpal::Stream, SourceInfo)> {
    let host = cpal::default_host();

    // A prioridade do usuário vence, mas só se o aparelho estiver conectado
    // agora. Não estando, cai no padrão do sistema — ficar sem gravar porque um
    // fone está na gaveta seria pior que gravar pelo microfone errado.
    let device = preferred
        .and_then(|wanted| {
            host.input_devices()
                .ok()?
                .find(|device| device.name().is_ok_and(|name| name == wanted))
        })
        .or_else(|| host.default_input_device())
        .ok_or_else(|| anyhow!("nenhum microfone disponível"))?;

    let device_name = device.name().unwrap_or_else(|_| "desconhecido".into());
    let config = device
        .default_input_config()
        .context("configuração padrão da entrada")?;

    let source_rate = config.sample_rate().0;
    let source_channels = config.channels();
    let sample_format = config.sample_format();
    let stream_config: StreamConfig = config.into();

    let ratio = source_rate as f64 / TARGET_RATE as f64;

    let stream = match sample_format {
        SampleFormat::F32 => input_stream::<f32>(&device, &stream_config, sink, source_channels, ratio),
        SampleFormat::I16 => input_stream::<i16>(&device, &stream_config, sink, source_channels, ratio),
        SampleFormat::U16 => input_stream::<u16>(&device, &stream_config, sink, source_channels, ratio),
        other => return Err(anyhow!("formato de amostra não suportado: {other}")),
    }?;

    Ok((
        stream,
        SourceInfo {
            device_name,
            source_rate,
            source_channels,
        },
    ))
}

fn input_stream<S>(
    device: &cpal::Device,
    config: &StreamConfig,
    sink: Arc<Mutex<Shared>>,
    channels: u16,
    ratio: f64,
) -> Result<cpal::Stream>
where
    S: SizedSample,
    f32: FromSample<S>,
{
    // O resto entre blocos precisa sobreviver de callback a callback, senão a
    // reamostragem acumula erro de fase e o áudio sai com estalos.
    let mut position = 0.0f64;

    device
        .build_input_stream(
            config,
            move |input: &[S], _| {
                push_samples(&sink, input, channels, ratio, &mut position);
            },
            |err| tracing::error!(?err, "erro no stream de entrada"),
            None,
        )
        .context("abrir o stream de entrada")
}

/// Mistura para mono, reamostra para 16 kHz e acumula — tudo num passe só.
fn push_samples<S>(
    sink: &Arc<Mutex<Shared>>,
    input: &[S],
    channels: u16,
    ratio: f64,
    position: &mut f64,
) where
    S: SizedSample,
    f32: FromSample<S>,
{
    let channels = channels.max(1) as usize;
    let frames = input.len() / channels;
    if frames == 0 {
        return;
    }

    let mut shared = sink.lock();

    while (*position) < frames as f64 {
        let base = (*position as usize) * channels;

        let mut mixed = 0.0f32;
        for offset in 0..channels {
            mixed += f32::from_sample_(input[base + offset]);
        }
        mixed /= channels as f32;

        let magnitude = mixed.abs();
        if magnitude > shared.peak {
            shared.peak = magnitude;
        }
        if magnitude > SPEECH_THRESHOLD {
            shared.speech_detected = true;
        }

        shared.samples.push((mixed.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);

        if shared.samples.len() % LEVEL_WINDOW == 0 {
            let start = shared.samples.len() - LEVEL_WINDOW;
            let window_peak = shared.samples[start..]
                .iter()
                .map(|sample| (*sample as f32 / i16::MAX as f32).abs())
                .fold(0.0f32, f32::max);
            shared.envelope.push(window_peak);
        }

        *position += ratio;
    }

    *position -= frames as f64;
}
