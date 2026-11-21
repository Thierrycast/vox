/* Testes da montagem de pedidos e da leitura de respostas — o que dá para
 * provar sem sessão nem rede. `pedidos.js` não sabe que Claude Code existe:
 * entra config, sai {url, init}; entra resposta, sai JSON ou frase de erro.
 *
 * O contrato que interessa é o das mensagens: um 401 tem que dizer "token",
 * um 429 "fila cheia", um 403 apontar para as preferências — quem está
 * configurando decide com essa frase, não com um número de status.
 */
import { expect, test } from 'claude-code/testing'
import * as ponte from './pedidos.js'

interface Config {
  host: string
  token: string
  mode: string
  label: string
}

interface Resposta {
  status: number
  ok: boolean
  text: string
}

const CONFIG: Config = {
  host: 'http://127.0.0.1:8765',
  token: 'segredo',
  mode: 'speak',
  label: 'claude',
}

function respostaDe(status: number, corpo?: string): Resposta {
  return { status, ok: status < 400, text: corpo ?? '' }
}

function falha(promessa: Promise<unknown>): Promise<Error> {
  return promessa.then(
    (valor) => new Error(`devolveu ${JSON.stringify(valor)} em vez de falhar`),
    (erro: Error) => erro,
  )
}

test('montarNarracao manda texto, origem e modo com o token', () => {
  const { url, init } = ponte.montarNarracao(CONFIG, 'resposta do agente')

  expect(url).toBe('http://127.0.0.1:8765/narrate')
  expect(init.method).toBe('POST')
  expect(init.headers['X-Vox-Token']).toBe('segredo')
  const corpo = JSON.parse(String(init.body)) as Record<string, string>
  expect(corpo.text).toBe('resposta do agente')
  expect(corpo.source).toBe('claude')
  expect(corpo.mode).toBe('speak')
})

test('host com barra no fim não vira rota quebrada', () => {
  const { url } = ponte.montarNarracao({ ...CONFIG, host: 'http://100.1.2.3:8765/' }, 'x')
  expect(url).toBe('http://100.1.2.3:8765/narrate')
})

test('montarFila é leitura: sem método, sem corpo', () => {
  const { url, init } = ponte.montarFila(CONFIG)
  expect(url).toBe('http://127.0.0.1:8765/narrate/queue')
  expect(init.method ?? 'GET').toBe('GET')
  expect(init.body ?? null).toBe(null)
  expect(init.headers['X-Vox-Token']).toBe('segredo')
})

test('tocar e cancelar carregam o id no corpo', () => {
  for (const montado of [ponte.montarTocar(CONFIG, 7), ponte.montarCancelar(CONFIG, 7)]) {
    expect((JSON.parse(String(montado.init.body)) as Record<string, number>).id).toBe(7)
  }
})

test('interpretar devolve o JSON quando deu certo', async () => {
  const corpo = await ponte.interpretar(respostaDe(200, '{"ok":true,"id":3}'), ponte.MENSAGENS.narrar)
  expect((corpo as Record<string, number>).id).toBe(3)
})

test('interpretar aceita corpo vazio sem engasgar', async () => {
  const corpo = await ponte.interpretar(respostaDe(200, ''), {})
  expect(corpo).toEqual({})
})

test('401 diz o que fazer com o token', async () => {
  const erro = await falha(ponte.interpretar(respostaDe(401), ponte.MENSAGENS.narrar))
  expect(erro.message).toContain('token')
})

test('429 explica a fila cheia', async () => {
  const erro = await falha(ponte.interpretar(respostaDe(429), ponte.MENSAGENS.narrar))
  expect(erro.message).toContain('fila')
})

test('403 aponta para as preferências do Vox', async () => {
  const erro = await falha(ponte.interpretar(respostaDe(403), ponte.MENSAGENS.narrar))
  expect(erro.message).toContain('preferências')
})

test('o detalhe do servidor ganha a frente de um status seco', async () => {
  const erro = await falha(ponte.interpretar(respostaDe(500, '{"detail":"abrir a porta falhou"}'), ponte.MENSAGENS.simples))
  expect(erro.message).toContain('abrir a porta')
})

test('o erro do servidor também serve quando a frase própria falta', async () => {
  const erro = await falha(ponte.interpretar(respostaDe(404, '{"error":"nada tocando"}'), ponte.MENSAGENS.simples))
  expect(erro.message).toBe('nada tocando')
})
