/* Os pedidos à ponte do Vox, prontos para serem enviados.
 *
 * A análise estática do sistema de Mods só deixa a API (`$.http.fetch`,
 * `$.store`) ser **chamada**, literalmente, dentro dos hooks — nunca passada
 * adiante como valor, nem como `$.http.fetch`, nem como a própria sessão `$`.
 * Então este módulo não sabe que Claude Code existe: ele monta o pedido
 * (`montar*`) e interpreta a resposta (`interpretar`), e quem chama é quem tem
 * `$` na mão.
 *
 * Todas as mensagens de erro são frases em português que cabem no terminal:
 * quem está configurando decide com elas, e um "401" sozinho não diz se o
 * problema é token, endereço ou narração desligada.
 */

export function cabecalhos(token) {
  return { 'X-Vox-Token': token }
}

function url(config, rota) {
  return `${config.host.replace(/\/$/, '')}${rota}`
}

function post(token, corpo) {
  return {
    method: 'POST',
    headers: { ...cabecalhos(token), 'Content-Type': 'application/json' },
    body: JSON.stringify(corpo),
  }
}

/** Enfileira uma resposta para narração. */
export function montarNarracao(config, texto) {
  return {
    url: url(config, '/narrate'),
    init: post(config.token, { text: texto, source: config.label, mode: config.mode }),
  }
}

/** A fila inteira: corrente e pendentes, para o painel desenhar. */
export function montarFila(config) {
  return { url: url(config, '/narrate/queue'), init: { headers: cabecalhos(config.token) } }
}

/** Manda um item tocar: o pronto no topo, ou promove um pendente. */
export function montarTocar(config, id) {
  return { url: url(config, '/narrate/play'), init: post(config.token, { id }) }
}

/** Cancela um item da fila — ou o que está tocando. */
export function montarCancelar(config, id) {
  return { url: url(config, '/narrate/cancel'), init: post(config.token, { id }) }
}

/** Para o corrente e avança para o próximo. */
export function montarPular(config) {
  return { url: url(config, '/narrate/skip'), init: post(config.token, {}) }
}

/** pause | resume | stop — o transporte da fala. */
export function montarControle(config, acao) {
  return { url: url(config, `/${acao}`), init: post(config.token, {}) }
}

/** Frases por status, para os erros que o Vox já explica por si. */
export const MENSAGENS = {
  narrar: {
    401: 'o token não bate. Copie de novo o token da ponte (preferências do Vox, seção Extensão) e rode /vox token <valor>.',
    403: 'a narração está desligada no Vox (preferências, seção Narração) — ou o serviço está em pausa.',
    429: 'a fila de narração do Vox está cheia. Cancele um item no painel ou espere a fila andar.',
  },
  fila: {
    401: 'o token não bate. Copie de novo o token da ponte (preferências do Vox, seção Extensão) e rode /vox token <valor>.',
    403: 'a narração está desligada no Vox (preferências, seção Narração) — ou o serviço está em pausa.',
  },
  item: {
    404: 'este item já tocou ou foi cancelado.',
  },
  pular: {
    404: 'não há narração tocando para pular.',
  },
  simples: {},
}

/** Lê a resposta: o JSON de sucesso, ou `Error` com a frase do caso. */
export async function interpretar(resposta, mensagens) {
  if (resposta.ok) return JSON.parse(resposta.text || '{}')
  const detalhe = mensagens[resposta.status] ?? detalheDoServidor(resposta)
  throw new Error(detalhe)
}

function detalheDoServidor(resposta) {
  try {
    const corpo = JSON.parse(resposta.text || '{}')
    if (corpo.detail) return corpo.detail
    if (corpo.error) return corpo.error
  } catch {}
  return `o Vox respondeu ${resposta.status}`
}
