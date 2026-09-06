//! Retorno sonoro.
//!
//! Os sons são o canal principal quando o usuário não está olhando para a tela,
//! então duas coisas importam mais que o resto:
//!
//! - **Tocar rápido.** Os arquivos vão embutidos no binário e a saída de áudio
//!   fica aberta desde a partida; no momento do gatilho não há leitura de disco
//!   nem abertura de dispositivo.
//! - **Serem distinguíveis sem olhar.** O conjunto se separa por *contorno*, não
//!   por altura: só um sobe, só um bate dissonante, só um é curtíssimo.
//!
//! A gramática veio de medir os sons de ditado do Raycast (senoide com harmônico
//! de oitava, ataque de 12–55 ms, decaimento exponencial puro) e o significado
//! mora no intervalo: quinta justa resolve, segunda menor dá erro.
//!
//! ## Por que existe uma thread
//!
//! `rodio::OutputStream` embrulha um `cpal::Stream`, que não é `Send` nem
//! `Sync`. Como o `SoundBank` vive dentro do estado compartilhado do Tauri, a
//! saída de áudio mora numa thread própria e recebe os bytes por canal. O que
//! atravessa fronteira de thread é só `Vec<u8>`.

use std::io::Cursor;
use std::sync::mpsc::{self, Sender};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rodio::{Decoder, OutputStream, Sink};

/// Cada evento sonoro do app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// Ditado começou. A5 sozinho.
    DictationStart,
    /// Texto entregue. Quinta justa, resolve.
    DictationSuccess,
    /// Falha do ditado. Segunda menor sobre E, bate.
    DictationFailure,
    /// Leitura começou. D5 → A5, **sobe**.
    SpeakStart,
    /// Leitura terminou. A5 cai e resolve em D5, **desce**.
    SpeakComplete,
    /// Leitura pausada. Blip curto de D5.
    SpeakPause,
    /// Leitura falhou. D5 + C#5, **bate**.
    SpeakError,
}

impl Cue {
    /// Os quatro sons da leitura são nossos e viajam com o binário.
    ///
    /// Os três do ditado são arquivos do Raycast e ficam de fora de propósito —
    /// `load_external_dictation_sounds` aponta para os `.aif` no disco de quem já
    /// os tem instalados; sem isso, o ditado toca os equivalentes da leitura.
    fn embedded(self) -> Option<&'static [u8]> {
        match self {
            Cue::SpeakStart => Some(include_bytes!("../../assets/sounds/speak-start.wav")),
            Cue::SpeakComplete => Some(include_bytes!("../../assets/sounds/speak-complete.wav")),
            Cue::SpeakPause => Some(include_bytes!("../../assets/sounds/speak-pause.wav")),
            Cue::SpeakError => Some(include_bytes!("../../assets/sounds/speak-error.wav")),
            Cue::DictationStart | Cue::DictationSuccess | Cue::DictationFailure => None,
        }
    }

    /// Para onde cair quando o som próprio do evento não existe.
    fn fallback(self) -> Option<Cue> {
        match self {
            Cue::DictationStart => Some(Cue::SpeakStart),
            Cue::DictationSuccess => Some(Cue::SpeakComplete),
            Cue::DictationFailure => Some(Cue::SpeakError),
            _ => None,
        }
    }

    fn external_name(self) -> Option<&'static str> {
        match self {
            Cue::DictationStart => Some("modern-start.aif"),
            Cue::DictationSuccess => Some("modern-success.aif"),
            Cue::DictationFailure => Some("modern-failure.aif"),
            _ => None,
        }
    }
}

pub struct SoundBank {
    /// Canal para a thread que detém a saída de áudio.
    player: Sender<Vec<u8>>,
    external: Mutex<Vec<(Cue, Vec<u8>)>>,
    enabled: Mutex<bool>,
}

impl SoundBank {
    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        std::thread::Builder::new()
            .name("vox-sound".into())
            .spawn(move || {
                // O stream nasce e morre nesta thread; nunca atravessa fronteira.
                let (stream, handle) = match OutputStream::try_default() {
                    Ok(pair) => {
                        let _ = ready_tx.send(Ok(()));
                        pair
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err.to_string()));
                        return;
                    }
                };

                // Fica escutando até o canal fechar, o que só acontece quando o
                // app encerra.
                while let Ok(bytes) = rx.recv() {
                    let sink = match Sink::try_new(&handle) {
                        Ok(sink) => sink,
                        Err(err) => {
                            tracing::warn!(?err, "não foi possível criar o sink de áudio");
                            continue;
                        }
                    };
                    match Decoder::new(Cursor::new(bytes)) {
                        // `detach` deixa o som terminar sozinho; o stink morre
                        // com o stream, que vive enquanto o app viver.
                        Ok(decoded) => {
                            sink.append(decoded);
                            sink.detach();
                        }
                        Err(err) => tracing::warn!(?err, "falha ao decodificar som"),
                    }
                }

                drop(stream);
            })
            .context("subir a thread de som")?;

        ready_rx
            .recv()
            .context("a thread de som terminou sem responder")?
            .map_err(anyhow::Error::msg)
            .context("abrir a saída de áudio")?;

        Ok(Self {
            player: tx,
            external: Mutex::new(Vec::new()),
            enabled: Mutex::new(true),
        })
    }

    pub fn set_enabled(&self, enabled: bool) {
        *self.enabled.lock() = enabled;
    }

    /// Carrega os sons do ditado de uma pasta no disco, se ela existir.
    ///
    /// Pensado para apontar ao `Resources/Audio/Dictation` de uma instalação do
    /// Raycast. Ausência não é erro: o app usa os equivalentes próprios.
    pub fn load_external_dictation_sounds(&self, directory: &std::path::Path) -> usize {
        let mut bank = self.external.lock();
        bank.clear();

        for cue in [Cue::DictationStart, Cue::DictationSuccess, Cue::DictationFailure] {
            let Some(name) = cue.external_name() else { continue };
            let path = directory.join(name);
            match std::fs::read(&path) {
                Ok(bytes) => bank.push((cue, bytes)),
                Err(err) => tracing::debug!(?path, ?err, "som externo ausente"),
            }
        }

        let loaded = bank.len();
        tracing::info!(loaded, ?directory, "sons de ditado carregados do disco");
        loaded
    }

    /// Dispara um som sem bloquear quem chamou.
    ///
    /// Falha de áudio nunca interrompe o fluxo: perder um bipe é irrelevante
    /// perto de perder a transcrição.
    pub fn play(&self, cue: Cue) {
        if !*self.enabled.lock() {
            tracing::debug!(?cue, "som suprimido: retorno sonoro desligado nas preferências");
            return;
        }

        let bytes = self
            .external
            .lock()
            .iter()
            .find(|(kind, _)| *kind == cue)
            .map(|(_, bytes)| bytes.clone())
            .or_else(|| cue.embedded().map(<[u8]>::to_vec))
            .or_else(|| cue.fallback()?.embedded().map(<[u8]>::to_vec));

        let Some(bytes) = bytes else {
            tracing::warn!(?cue, "sem som disponível para este evento");
            return;
        };

        tracing::debug!(?cue, bytes = bytes.len(), "enfileirando som");
        if self.player.send(bytes).is_err() {
            tracing::warn!(?cue, "a thread de som morreu; nenhum som será tocado");
        }
    }
}
