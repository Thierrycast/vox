/* Transcrição ao vivo — janelinha do canto superior direito.
 *
 * Só escuta. Toda decisão é do backend: ele decide se a janela aparece (a
 * preferência `show_live_transcription`) e ele publica o texto. Aqui não há
 * poll nem estado próprio.
 */

const { listen } = window.__TAURI__.event;

const card = document.getElementById("card");
const text = document.getElementById("text");

listen("vox://live-text", (event) => {
  const reconhecido = event.payload?.text ?? "";
  text.textContent = reconhecido;
  // A entrada é animada; por isso o atributo entra num quadro seguinte e não
  // junto com o primeiro texto.
  requestAnimationFrame(() => card.setAttribute("data-visible", ""));
});

/* O backend esconde a janela ao fim do ditado. Limpar aqui evita que o texto
   da sessão anterior pisque no começo da próxima. */
listen("vox://hud", (event) => {
  const state = event.payload?.state;
  if (state && state !== "recording") {
    card.removeAttribute("data-visible");
    text.textContent = "";
  }
});
