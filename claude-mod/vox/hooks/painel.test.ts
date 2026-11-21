import { expect, test } from 'claude-code/testing'
import { comAtual, mascararToken, opcoesDeMicrofone, opcoesDeVoz, opcoesNumericas, secaoEfetiva, SECOES } from './painel.js'

const PAINEL = {
  plugin: 'vox',
  component: 'Pane',
  requestId: 'vox-painel',
  props: {
    title: 'Vox',
    isFocused: true,
    bodyColumns: 56,
    placement: 'dock',
    scroll: { offset: 0, bodyRows: 40 },
    view: {},
  },
} as const

const CONFIG = { host: 'http://127.0.0.1:8765', token: 'segredo-1234', auto: true, mode: 'speak', label: 'claude' }

const PREFERENCIAS = {
  service_enabled: true,
  start_with_windows: false,
  sounds_enabled: true,
  sounds_volume: 0.7,
  theme_accent: '#966aff',
  voice: 'piper:pt_BR-cadu-medium',
  speed: 1,
  prebuffer_ratio: 0.1,
  normalize_before_reading: false,
  reading_rewrite_enabled: true,
  narrate_tables: true,
  open_reader_on_read: false,
  narration_enabled: true,
  narration_auto_speak: true,
  narration_model: null,
  narration_queue_limit: 10,
  input_device: null,
  transcription_model: 'groq/whisper-large-v3-turbo',
  output_action: 'paste',
  submit_mode: 'keyword',
  submit_keyword: 'manda ver',
  submit_key: 'enter',
  live_transcription: true,
  show_live_transcription: false,
  stt_rewrite_enabled: true,
  stt_rewrite_preset: 'fala-limpa',
  stt_rewrite_intensity: 2,
  stt_rewrite_model: null,
  vocabulary: ['Tauri', 'Traefik'],
  custom_instructions: '',
  hud_placement_dictation: 'livre',
  hud_placement_reading: 'meio-direita',
  shortcuts: { 'read-selection': 'Ctrl+Alt+L' },
  bridge_enabled: true,
  bridge_port: 8765,
  bridge_tailscale: false,
  reading_captions: false,
  api_base_url: 'http://servidor:8010',
  api_auth: false,
  settings_path: 'C:/vox/settings.json',
}

const CATALOGO = {
  voices: { default: 'piper:pt_BR-cadu-medium', custom: [], base: ['piper:pt_BR-cadu-medium', 'kokoro:pf_dora'] },
  rewrite_presets: { presets: [{ id: 'fala-limpa', label: 'Fala limpa', description: '' }] },
  input_devices: [{ id: 'mic-1', name: 'Headset', is_default: true }],
  commands: [
    { id: 'read-selection', label: 'Ler seleção', hint: 'lê o texto selecionado', default_binding: 'Ctrl+Alt+L', binding: 'Ctrl+Alt+L', failure: null },
    { id: 'dictate', label: 'Ditar', hint: '', default_binding: 'Ctrl+Alt+D', binding: 'Ctrl+Alt+D', failure: 'tomado por outro app' },
  ],
  transcription_models: [
    { value: 'groq/whisper-large-v3-turbo', label: 'Whisper large v3 turbo' },
    { value: 'vosk', label: 'Vosk — local' },
  ],
}

const FILA_VAZIA = { enabled: true, service_enabled: true, reading_state: 'Idle', current: null, items: [] }
const FILA_CHEIA = {
  enabled: true,
  service_enabled: true,
  reading_state: 'Playing',
  current: { id: 7, source: 'claude', preview: 'tocando agora', state: 'tocando', mode: 'speak' },
  items: [{ id: 8, source: 'claude', preview: 'a próxima', state: 'pronto', mode: 'hold' }],
}

type Pedido = { url: string; corpo: unknown }

function vox(on: Parameters<Parameters<typeof test>[1]>[1], fila: unknown, pedidos: Pedido[], online = true) {
  let preferencias: Record<string, unknown> = { ...PREFERENCIAS }
  on('store.get', () => ({ value: CONFIG }))
  on('store.set', () => ({ value: undefined }))
  on('session.start', (_: unknown, e: { cwd: string }) => ({ cwd: e.cwd }))
  on('clock.every', () => ({ value: undefined }))
  on('clock.after', () => ({ value: undefined }))
  on('command.register', () => ({ value: { command: 'vox' } }))
  on('ui.open', () => ({ value: { isPlaced: true } }))
  on('ui.panes', () => ({ value: [] }))
  on('http.fetch', (_: unknown, e: { url: string; init?: { body?: string } }) => {
    const corpo = e.init?.body ? JSON.parse(e.init.body) as unknown : undefined
    pedidos.push({ url: e.url, corpo })
    if (!online) return { value: { status: 502, ok: false, text: '', headers: {} } }
    const rota = new URL(e.url).pathname
    const responder = (dado: unknown) => ({ value: { status: 200, ok: true, text: JSON.stringify(dado), headers: {} } })
    if (rota === '/narrate/queue') return responder(fila)
    if (rota === '/settings' && corpo) {
      preferencias = { ...preferencias, ...(corpo as Record<string, unknown>) }
      return responder(preferencias)
    }
    if (rota === '/settings') return responder(preferencias)
    if (rota === '/settings/catalog') return responder(CATALOGO)
    return responder({ ok: true })
  })
}

async function abrir($: Parameters<Parameters<typeof test>[1]>[0]) {
  await $.session.start({ cwd: '/', surface: 'terminal', isInteractive: true })
  await $.command.run({ command: 'vox', args: '', origin: 'person', presentation: 'default' } as never)
}

test('nada tocando não quebra o painel: current nulo vira "nada tocando."', async ($, on) => {
  const pedidos: Pedido[] = []
  vox(on, FILA_VAZIA, pedidos)
  await abrir($)
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...PAINEL, surface })
    expect((await ui.find({ type: 'Text', text: /nada tocando/ }))?.text).toBe('nada tocando.')
    await ui.unmount()
  }
})

test('cada aba desenha com o Vox conectado', async ($, on) => {
  const pedidos: Pedido[] = []
  vox(on, FILA_CHEIA, pedidos)
  await abrir($)
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...PAINEL, surface })
    await ui.press({ key: 'testar' }).catch(() => undefined)
    for (const secao of SECOES) {
      await ui.press({ key: `aba-${secao.id}` })
      expect(await ui.find({ key: 'raiz' })).toBeDefined()
    }
    await ui.unmount()
  }
})

test('cada aba desenha sem conexão, e aponta para a aba conexão', async ($, on) => {
  const pedidos: Pedido[] = []
  vox(on, FILA_VAZIA, pedidos, false)
  await abrir($)
  const ui = await $.ui.mount({ ...PAINEL, surface: 'terminal' })
  expect(await ui.find({ key: 'mod-host' })).toBeDefined()
  for (const secao of SECOES) {
    await ui.press({ key: `aba-${secao.id}` })
    expect(await ui.find({ key: 'raiz' })).toBeDefined()
  }
  await ui.press({ key: 'aba-leitura' })
  expect(await ui.find({ type: 'Text', text: /sem conexão com o Vox/ })).toBeDefined()
  await ui.unmount()
})

test('fila: transporte, tocar e cancelar mandam o pedido certo', async ($, on) => {
  const pedidos: Pedido[] = []
  vox(on, FILA_CHEIA, pedidos)
  await abrir($)
  const ui = await $.ui.mount({ ...PAINEL, surface: 'terminal' })
  await ui.press({ key: 'aba-narracao' })
  await ui.press({ key: 'pausar' })
  await ui.press({ key: 'tocar-8' })
  await ui.press({ key: 'cancelar-8' })
  const rotas = pedidos.map((pedido) => new URL(pedido.url).pathname)
  expect(rotas).toContain('/pause')
  expect(rotas).toContain('/narrate/play')
  expect(rotas).toContain('/narrate/cancel')
  await ui.unmount()
})

test('preferência alternada vai como ajuste de um campo só', async ($, on) => {
  const pedidos: Pedido[] = []
  vox(on, FILA_VAZIA, pedidos)
  await abrir($)
  const ui = await $.ui.mount({ ...PAINEL, surface: 'terminal' })
  await ui.press({ key: 'aba-leitura' })
  await ui.press({ key: 'narrate-tables' })
  await ui.press({ key: 'reading-rewrite' })
  await ui.select({ key: 'speed', value: '1.25' })
  const ajustes = pedidos.filter((pedido) => new URL(pedido.url).pathname === '/settings' && pedido.corpo)
  expect(ajustes.map((pedido) => pedido.corpo)).toEqual([
    { narrate_tables: false },
    { reading_rewrite_enabled: false },
    { speed: 1.25 },
  ])
  await ui.unmount()
})

test('vocabulário: termo novo entra no fim, clique no termo tira', async ($, on) => {
  const pedidos: Pedido[] = []
  vox(on, FILA_VAZIA, pedidos)
  await abrir($)
  const ui = await $.ui.mount({ ...PAINEL, surface: 'terminal' })
  await ui.press({ key: 'aba-ditado' })
  await ui.input({ key: 'vocabulario-novo', text: 'Ghostty, Vosk' })
  await ui.press({ key: 'termo-Tauri' })
  const ajustes = pedidos.filter((pedido) => new URL(pedido.url).pathname === '/settings' && pedido.corpo)
  expect(ajustes.map((pedido) => pedido.corpo)).toEqual([
    { vocabulary: ['Tauri', 'Traefik', 'Ghostty', 'Vosk'] },
    { vocabulary: ['Traefik', 'Ghostty', 'Vosk'] },
  ])
  await ui.unmount()
})

test('atalho editado vai só com o comando mexido', async ($, on) => {
  const pedidos: Pedido[] = []
  vox(on, FILA_VAZIA, pedidos)
  await abrir($)
  const ui = await $.ui.mount({ ...PAINEL, surface: 'terminal' })
  await ui.press({ key: 'aba-conexao' })
  await ui.press({ key: 'testar' })
  await ui.press({ key: 'aba-atalhos' })
  expect(await ui.find({ type: 'Text', text: /tomado por outro app/ })).toBeDefined()
  await ui.input({ key: 'atalho-read-selection-campo', text: 'Ctrl+Alt+K' })
  const ajuste = pedidos.find((pedido) => new URL(pedido.url).pathname === '/settings' && pedido.corpo)
  expect(ajuste?.corpo).toEqual({ shortcuts: { 'read-selection': 'Ctrl+Alt+K' } })
  await ui.unmount()
})

test('o token nunca aparece inteiro no painel', async ($, on) => {
  const pedidos: Pedido[] = []
  vox(on, FILA_VAZIA, pedidos)
  await abrir($)
  const ui = await $.ui.mount({ ...PAINEL, surface: 'terminal' })
  await ui.press({ key: 'aba-conexao' })
  const desenhado = JSON.stringify(await ui.drawn())
  expect(desenhado.includes('segredo-1234')).toBe(false)
  expect(desenhado.includes('••••1234')).toBe(true)
  await ui.unmount()
})

test('opções mantêm o valor atual mesmo fora da lista', () => {
  expect(comAtual([{ value: 'a' }], 'b')[0]).toEqual({ value: 'b', label: 'b' })
  expect(opcoesDeVoz(null, 'piper:x')).toEqual([{ value: 'piper:x', label: 'piper:x' }])
  expect(opcoesDeMicrofone({ input_devices: [] }, 'sumiu')[0]?.label).toBe('microfone ausente')
  const velocidade = opcoesNumericas([1, 1.5], 1.3, (n: number) => `${n}x`)
  expect(velocidade.valor).toBe('1.3')
  expect(velocidade.opcoes[0]).toEqual({ value: '1.3', label: '1.3x' })
})

test('âncora do widget vai como ajuste de um campo só', async ($, on) => {
  const pedidos: Pedido[] = []
  vox(on, FILA_VAZIA, pedidos)
  await abrir($)
  const ui = await $.ui.mount({ ...PAINEL, surface: 'terminal' })
  await ui.press({ key: 'aba-widget' })
  await ui.select({ key: 'ancora-ditado', value: 'baixo-direita' })
  const ajustes = pedidos.filter((pedido) => new URL(pedido.url).pathname === '/settings' && pedido.corpo)
  expect(ajustes.map((pedido) => pedido.corpo)).toEqual([{ hud_placement_dictation: 'baixo-direita' }])
  await ui.unmount()
})

test('token mascarado e aba padrão conforme a conexão', () => {
  expect(mascararToken('')).toBe('nenhum')
  expect(mascararToken('abcdef9876')).toBe('••••9876')
  expect(secaoEfetiva(null, 'ok')).toBe('narracao')
  expect(secaoEfetiva(null, 'fora-do-ar')).toBe('conexao')
  expect(secaoEfetiva('ditado', 'fora-do-ar')).toBe('ditado')
})
