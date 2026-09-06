//! Guarda contra string de JavaScript partida por escape perdido.
//!
//! Já aconteceu: um `\n` dentro de uma string virou quebra de linha de verdade
//! ao escrever o arquivo. O módulo inteiro deixa de parsear, e como não há
//! console nessas janelas a falha é *muda* — o CSS desenha a moldura e nada
//! mais acontece. Custou horas de diagnóstico às cegas.
//!
//! O teste não é um parser de JavaScript: ele só cobra que cada linha feche as
//! aspas duplas que abriu, que é exatamente a assinatura dessa falha.

use std::fs;
use std::path::Path;

/// Aspas duplas não escapadas na linha, ignorando o que está entre crases
/// (template literal pode legitimamente ocupar várias linhas).
fn aspas_abertas(linha: &str) -> usize {
    const BARRA: u32 = 92; // evita escrever a contrabarra literal aqui
    let caracteres: Vec<char> = linha.chars().collect();
    let (mut total, mut i, mut em_crase) = (0usize, 0usize, false);

    while i < caracteres.len() {
        let atual = caracteres[i];
        if atual as u32 == BARRA {
            i += 2;
            continue;
        }
        if atual == '`' {
            em_crase = !em_crase;
        } else if atual == '"' && !em_crase {
            total += 1;
        }
        i += 1;
    }
    total
}

#[test]
fn strings_do_front_nao_estao_partidas() {
    let raiz = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/js");
    let mut quebradas = Vec::new();

    for entrada in fs::read_dir(&raiz).expect("pasta ../src/js") {
        let caminho = entrada.expect("entrada de diretório").path();
        if caminho.extension().and_then(|e| e.to_str()) != Some("js") {
            continue;
        }
        let conteudo = fs::read_to_string(&caminho).expect("ler o arquivo");
        let nome = caminho.file_name().unwrap().to_string_lossy().to_string();

        for (indice, linha) in conteudo.lines().enumerate() {
            if aspas_abertas(linha) % 2 == 1 {
                quebradas.push(format!("{nome}:{}: {}", indice + 1, linha.trim()));
            }
        }
    }

    assert!(
        quebradas.is_empty(),
        "string aberta e não fechada na mesma linha — sinal de escape perdido:\n{}",
        quebradas.join("\n")
    );
}

#[test]
fn contador_de_aspas_reconhece_os_casos() {
    assert_eq!(aspas_abertas(r#"const a = "ok";"#), 2);
    assert_eq!(aspas_abertas(r#"const a = "partida"#), 1);
    assert_eq!(aspas_abertas(r#"const a = `crase com " dentro`;"#), 0);
}
