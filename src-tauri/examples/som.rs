//! Diagnóstico de áudio: toca os sons do Vox sem subir o Vox.
//!
//! Existe porque toda falha de som no app cai em `tracing::debug!` e some no
//! nível `info` — o sintoma é silêncio, sem uma linha de log. Aqui a mesma
//! biblioteca abre o mesmo dispositivo e toca os mesmos arquivos, com o erro
//! impresso. Se isto toca e o app não, o problema está no app; se nem isto
//! toca, está no dispositivo de saída.
//!
//! ```
//! cargo run --example som
//! ```

use std::io::Cursor;
use std::time::Duration;

use rodio::{Decoder, OutputStream, Sink};

const SONS: [(&str, &[u8]); 4] = [
    ("speak-start", include_bytes!("../../assets/sounds/speak-start.wav")),
    ("speak-complete", include_bytes!("../../assets/sounds/speak-complete.wav")),
    ("speak-pause", include_bytes!("../../assets/sounds/speak-pause.wav")),
    ("speak-error", include_bytes!("../../assets/sounds/speak-error.wav")),
];

fn main() {
    let dispositivo = rodio::cpal::traits::HostTrait::default_output_device(&rodio::cpal::default_host());
    match &dispositivo {
        Some(saida) => {
            use rodio::cpal::traits::DeviceTrait;
            println!("dispositivo padrão: {:?}", saida.name());
        }
        None => println!("nenhum dispositivo de saída padrão — é isso"),
    }

    let (_stream, handle) = match OutputStream::try_default() {
        Ok(par) => par,
        Err(erro) => {
            println!("FALHOU ao abrir a saída: {erro}");
            return;
        }
    };
    println!("saída aberta");

    for (nome, bytes) in SONS {
        print!("tocando {nome} ({} bytes)... ", bytes.len());
        let sink = match Sink::try_new(&handle) {
            Ok(sink) => sink,
            Err(erro) => {
                println!("FALHOU ao criar o sink: {erro}");
                continue;
            }
        };
        match Decoder::new(Cursor::new(bytes.to_vec())) {
            Ok(decodificado) => {
                sink.append(decodificado);
                // Bloqueia de propósito: um som por vez, para dar para ouvir
                // qual é qual.
                sink.sleep_until_end();
                println!("ok");
            }
            Err(erro) => println!("FALHOU ao decodificar: {erro}"),
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    println!("fim");
}
