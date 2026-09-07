/* Opções da extensão.
 *
 * "Salvar e testar" salva e imediatamente bate na ponte. Salvar sem testar
 * deixaria o usuário descobrir que o token está errado só na hora em que
 * quisesse ler algo — e aí o erro aparece longe daqui, sem dizer o que fazer.
 */

const campoToken = document.getElementById("token");
const campoPorta = document.getElementById("porta");
const estado = document.getElementById("estado");

chrome.storage.sync.get({ token: "", port: 8765 }).then((guardado) => {
  campoToken.value = guardado.token;
  campoPorta.value = guardado.port;
});

document.getElementById("salvar").addEventListener("click", async () => {
  const token = campoToken.value.trim();
  const port = Number(campoPorta.value) || 8765;

  await chrome.storage.sync.set({ token, port });
  estado.textContent = "testando…";
  estado.className = "estado";

  try {
    const resposta = await fetch(`http://127.0.0.1:${port}/health`, {
      headers: { "X-Vox-Token": token },
    });

    if (resposta.status === 401) {
      mostrar("token recusado pelo Vox", false);
      return;
    }
    if (!resposta.ok) {
      mostrar(`o Vox respondeu ${resposta.status}`, false);
      return;
    }
    mostrar("salvo — o Vox respondeu", true);
  } catch {
    // Erro de rede aqui quer dizer, quase sempre, que o Vox não está aberto:
    // não há para quem a porta responder.
    mostrar("não achei o Vox nessa porta — ele está aberto?", false);
  }
});

function mostrar(mensagem, deuCerto) {
  estado.textContent = mensagem;
  estado.className = `estado ${deuCerto ? "ok" : "erro"}`;
}
