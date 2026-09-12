/* Opções da extensão.
 *
 * "Salvar e testar" salva e imediatamente bate na ponte. Salvar sem testar
 * deixaria o usuário descobrir que o token está errado só na hora em que
 * quisesse ler algo — e aí o erro aparece longe daqui, sem dizer o que fazer.
 *
 * ## Por que as mensagens são compridas
 *
 * Porque "o Vox respondeu 403" mandou uma pessoa investigar um container que não
 * tinha relação nenhuma com a recusa. Um código de status é um fato sobre o
 * protocolo; quem está configurando precisa de um fato sobre a **situação dela**:
 * o que deu errado, e qual é o próximo passo.
 */

const campoToken = document.getElementById("token");
const campoPorta = document.getElementById("porta");
const estado = document.getElementById("estado");

chrome.storage.sync.get({ token: "", port: 8765 }).then((guardado) => {
  campoToken.value = guardado.token;
  campoPorta.value = guardado.port;
});

/* Cada resposta possível vira uma frase que diz o que fazer.
 *
 * A mensagem do servidor entra quando existe: ela é mais específica do que
 * qualquer coisa que dê para adivinhar daqui. */
function explicar(status, corpo, porta) {
  const detalhe = corpo?.detail || corpo?.error;

  if (status === 401) {
    return {
      texto: detalhe
        ? `Token recusado — ${detalhe}`
        : "Token recusado. Copie de novo em Preferências → Extensão, no Vox.",
      ok: false,
    };
  }

  if (status === 403) {
    return {
      texto: detalhe
        ? `Pedido recusado — ${detalhe}`
        : "A ponte recusou a origem deste pedido. Atualize a extensão: " +
          "versões antigas do Vox recusavam o teste desta tela.",
      ok: false,
    };
  }

  if (status === 404) {
    return {
      texto:
        `Algo respondeu na porta ${porta}, mas não é o Vox — ele não conhece ` +
        "essa rota. Confira se a porta é a mesma de bridge_port.",
      ok: false,
    };
  }

  if (status >= 500) {
    return {
      texto: `O Vox respondeu ${status}: falha dele, não da configuração. ` +
             "O log está em %APPDATA%\\vox\\config\\vox.log.",
      ok: false,
    };
  }

  return { texto: `Resposta inesperada (${status}).`, ok: false };
}

document.getElementById("salvar").addEventListener("click", async () => {
  const token = campoToken.value.trim();
  const port = Number(campoPorta.value) || 8765;

  await chrome.storage.sync.set({ token, port });

  if (!token) {
    mostrar("Falta o token. Ele está em Preferências → Extensão, no Vox.", false);
    return;
  }

  estado.textContent = "testando…";
  estado.className = "estado";

  let resposta;
  try {
    resposta = await fetch(`http://127.0.0.1:${port}/health`, {
      headers: { "X-Vox-Token": token },
    });
  } catch {
    // Erro de rede aqui quer dizer, quase sempre, que não há ninguém ouvindo —
    // e as duas causas prováveis são diferentes o bastante para valer dizer as
    // duas: o app fechado, ou a ponte desligada nas preferências dele.
    mostrar(
      `Ninguém respondeu em 127.0.0.1:${port}. O Vox está aberto? ` +
      "Se estiver, veja se a ponte está ligada em Preferências → Extensão.",
      false,
    );
    return;
  }

  let corpo = null;
  try {
    corpo = await resposta.json();
  } catch {
    // Resposta sem JSON: seguimos só com o status.
  }

  if (resposta.ok) {
    const pausado = corpo?.service_enabled === false;
    mostrar(
      pausado
        ? "Conectado — mas o Vox está em pausa. Reative no ícone da bandeja."
        : "Salvo — o Vox respondeu.",
      !pausado,
    );
    return;
  }

  const { texto, ok } = explicar(resposta.status, corpo, port);
  mostrar(texto, ok);
});

function mostrar(mensagem, deuCerto) {
  estado.textContent = mensagem;
  estado.className = `estado ${deuCerto ? "ok" : "erro"}`;
}
