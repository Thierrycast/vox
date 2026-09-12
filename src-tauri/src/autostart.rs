//! Subir junto com o Windows.
//!
//! ## Por que o registro, e não a pasta Inicializar
//!
//! A pasta `shell:startup` guarda atalhos `.lnk`, e criar um `.lnk` de dentro do
//! app exige COM (`IShellLink`) — uma dependência nova e um punhado de código
//! inseguro para escrever um caminho. A chave `Run` do usuário guarda uma linha
//! de comando em texto, que é exatamente o que temos.
//!
//! ## Por que `reg.exe`, e não a API do registro
//!
//! Falar com o registro em Rust pede o crate `windows`, que é grande e entraria
//! no binário inteiro para três operações de string. O `reg.exe` acompanha o
//! Windows desde sempre, roda sem privilégio nenhum nesta chave (é a do usuário,
//! não a da máquina) e faz o mesmo trabalho.
//!
//! ## O que entra na chave
//!
//! O caminho do executável, e não o do atalho de desenvolvimento. No boot a
//! pessoa quer o aplicativo, não uma compilação: o `vox-dev.ps1` recompila
//! quando alguma fonte mudou, e isso abriria uma janela de build no login.
//!
//! A consequência é que o `.env` do repositório **não** é lido ao subir pelo
//! boot — quem carrega aquilo é o atalho de desenvolvimento. Hoje isso não muda
//! nada, porque o endereço no `.env` é o mesmo que o padrão compilado; se um dia
//! divergirem, o log da partida diz qual endereço está valendo.

use std::process::Command;

/// Onde o Windows procura o que abrir no login do usuário.
const CHAVE: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// O nome da entrada. Mudar isto deixa a entrada antiga órfã no registro.
const VALOR: &str = "Vox";

/// `CREATE_NO_WINDOW`: sem isto cada consulta pisca um console preto na tela.
#[cfg(windows)]
const SEM_JANELA: u32 = 0x0800_0000;

fn comando(argumentos: &[&str]) -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new("reg");
    cmd.args(argumentos);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(SEM_JANELA);
    }

    cmd.output()
}

/// O Vox está registrado para subir no login?
pub fn is_enabled() -> bool {
    match comando(&["query", CHAVE, "/v", VALOR]) {
        Ok(saida) => saida.status.success(),
        Err(err) => {
            tracing::warn!(?err, "não deu para consultar a inicialização automática");
            false
        }
    }
}

/// Liga ou desliga a partida no login.
///
/// O caminho é escrito entre aspas porque quase sempre tem espaço — o executável
/// vive em `C:\Users\<nome>\...`, e sem as aspas o Windows tentaria abrir o
/// primeiro pedaço até o espaço.
pub fn set(enabled: bool) -> Result<(), String> {
    if !enabled {
        let saida = comando(&["delete", CHAVE, "/v", VALOR, "/f"])
            .map_err(|err| format!("chamar o reg.exe: {err}"))?;

        // Apagar o que não existe devolve erro, e para nós é sucesso: o pedido
        // era "não suba no login", e ele já não subia.
        if saida.status.success() || !is_enabled() {
            tracing::info!("inicialização automática desligada");
            return Ok(());
        }
        return Err(String::from_utf8_lossy(&saida.stderr).trim().to_string());
    }

    let executavel = std::env::current_exe()
        .map_err(|err| format!("descobrir o caminho do executável: {err}"))?;
    let linha = format!("\"{}\"", executavel.display());

    let saida = comando(&["add", CHAVE, "/v", VALOR, "/t", "REG_SZ", "/d", &linha, "/f"])
        .map_err(|err| format!("chamar o reg.exe: {err}"))?;

    if !saida.status.success() {
        return Err(String::from_utf8_lossy(&saida.stderr).trim().to_string());
    }

    tracing::info!(caminho = %executavel.display(), "inicialização automática ligada");
    Ok(())
}

/// Alinha o registro com a preferência gravada.
///
/// Chamado na partida porque as duas podem divergir por fora: alguém limpa a
/// chave com um utilitário de inicialização, ou copia o `settings.json` para
/// outra máquina. A preferência é a intenção; o registro é o estado.
pub fn sync(desejado: bool) {
    if is_enabled() == desejado {
        return;
    }
    if let Err(motivo) = set(desejado) {
        tracing::warn!(motivo, desejado, "não deu para alinhar a inicialização automática");
    }
}
