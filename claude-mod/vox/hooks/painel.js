export const SECOES = [
  { id: 'narracao', rotulo: 'narração' },
  { id: 'leitura', rotulo: 'leitura' },
  { id: 'ditado', rotulo: 'ditado' },
  { id: 'atalhos', rotulo: 'atalhos' },
  { id: 'geral', rotulo: 'geral' },
  { id: 'conexao', rotulo: 'conexão' },
]

export const CORES_DE_DESTAQUE = [
  { value: '#966aff', label: 'roxo' },
  { value: '#4f8cff', label: 'azul' },
  { value: '#37d67a', label: 'verde' },
  { value: '#f5a04a', label: 'âmbar' },
  { value: '#f4585c', label: 'vermelho' },
  { value: '#c9c9d4', label: 'neutro' },
]

export const FRASE_DE_AMOSTRA = 'Esta é a voz do Vox, na velocidade escolhida.'

const CLARO = '#e8e8e8'
const VERMELHO = '#ff6b6b'

const ESTADOS_DO_ITEM = {
  pendente: 'na fila',
  adaptando: 'adaptando…',
  pronto: 'pronta',
  tocando: 'tocando',
}

const COR_DO_ESTADO = {
  adaptando: 'warning',
  pronto: 'suggestion',
  tocando: 'success',
}

const STATUS = {
  ok: { cor: 'success', texto: 'conectado' },
  token: { cor: 'warning', texto: 'token não bate' },
  'fora-do-ar': { cor: 'error', texto: 'Vox fora do ar' },
  'sem-config': { cor: 'subtle', texto: 'sem token' },
}

const lit = (chave, cor = CLARO) => ({ scope: `vox:${chave}`, color: cor, dimColor: false, bold: true })

export function cortar(texto, maximo) {
  const valor = String(texto ?? '')
  const teto = Math.max(4, maximo)
  return valor.length > teto ? `${valor.slice(0, teto - 1)}…` : valor
}

export function mascararToken(token) {
  const valor = String(token ?? '')
  if (!valor) return 'nenhum'
  return valor.length <= 4 ? '••••' : `••••${valor.slice(-4)}`
}

export function comAtual(opcoes, atual, rotulo) {
  const valor = atual === null || atual === undefined ? '' : String(atual)
  if (opcoes.some((opcao) => opcao.value === valor)) return opcoes
  return [{ value: valor, label: rotulo ?? (valor || '—') }, ...opcoes]
}

export function opcoesDeVoz(catalogo, atual) {
  const vozes = catalogo?.voices
  const nomes = [...(vozes?.custom ?? []), ...(vozes?.base ?? [])]
  const unicos = [...new Set(nomes)]
  const opcoes = unicos.map((nome) => ({
    value: nome,
    label: nome === vozes?.default ? `${nome} (padrão do servidor)` : nome,
  }))
  return comAtual(opcoes, atual)
}

export function opcoesDeMicrofone(catalogo, atual) {
  const lista = Array.isArray(catalogo?.input_devices) ? catalogo.input_devices : []
  const opcoes = [
    { value: '', label: 'padrão do sistema' },
    ...lista.map((aparelho) => ({
      value: aparelho.id,
      label: aparelho.is_default ? `${aparelho.name} (padrão)` : aparelho.name,
    })),
  ]
  return comAtual(opcoes, atual ?? '', 'microfone ausente')
}

export function opcoesDePreset(catalogo, atual) {
  const lista = Array.isArray(catalogo?.rewrite_presets?.presets) ? catalogo.rewrite_presets.presets : []
  const opcoes = lista.map((preset) => ({ value: preset.id, label: preset.label ?? preset.id }))
  return comAtual(opcoes, atual)
}

export function opcoesDeModelo(catalogo, atual) {
  const lista = Array.isArray(catalogo?.transcription_models) ? catalogo.transcription_models : []
  return comAtual(lista.map((modelo) => ({ value: modelo.value, label: modelo.label })), atual)
}

export function opcoesNumericas(valores, atual, formatar) {
  const proximo = (valor) => Math.abs(Number(valor) - Number(atual)) < 1e-6
  const opcoes = valores.map((valor) => ({ value: String(valor), label: formatar(valor) }))
  const marcado = valores.find(proximo)
  if (marcado !== undefined) return { opcoes, valor: String(marcado) }
  return { opcoes: comAtual(opcoes, atual, formatar(Number(atual))), valor: String(atual) }
}

const porcento = (valor) => `${Math.round(Number(valor) * 100)}%`
const vezes = (valor) => `${Number(valor).toFixed(2).replace('.', ',')}×`

export function secaoEfetiva(secao, conexao) {
  if (SECOES.some((item) => item.id === secao)) return secao
  return conexao === 'ok' ? 'narracao' : 'conexao'
}

function linhaDivisoria(ui, chave, colunas) {
  return ui.Text({ key: chave, dimColor: true, children: '─'.repeat(Math.max(10, Math.min(colunas - 2, 48))) })
}

function titulo(ui, chave, texto) {
  return ui.Box({ key: chave, marginTop: 1, children: [ui.Text({ key: `${chave}-t`, dimColor: true, bold: true, children: texto })] })
}

function dica(ui, chave, texto) {
  return ui.Text({ key: chave, dimColor: true, wrap: 'wrap', children: texto })
}

function alternancia(ui, chave, rotulo, ligado, acao) {
  return ui.Button({ key: chave, plain: true, hover: lit(chave), onPress: acao, children: [
    ui.Text({ key: `${chave}-ponto`, color: ligado ? 'success' : undefined, dimColor: !ligado, children: ligado ? '●' : '○' }),
    ui.Text({ key: `${chave}-rotulo`, dimColor: !ligado, children: `  ${rotulo}` }),
  ] })
}

function escolha(ui, chave, rotulo, opcoes, valor, acao) {
  return ui.Select({ key: chave, label: rotulo, options: opcoes, value: valor, onSelect: acao })
}

function campo(ui, chave, rotulo, valor, acao, extra = {}) {
  return ui.Input({ key: chave, label: rotulo, value: valor ?? '', submitLabel: 'salvar', onSubmit: acao, ...extra })
}

function botao(ui, chave, rotulo, acao, extra = {}) {
  return ui.Button({ key: chave, hover: lit(chave), onPress: acao, label: rotulo, ...extra })
}

function semVox(p, chave) {
  const { ui, acoes } = p
  return ui.Box({ key: chave, flexDirection: 'column', marginTop: 1, children: [
    ui.Text({ key: `${chave}-t`, color: 'warning', wrap: 'wrap', children: 'sem conexão com o Vox: estas preferências moram no app.' }),
    ui.Box({ key: `${chave}-acoes`, marginTop: 1, children: [
      botao(ui, `${chave}-ir`, 'ir para conexão', () => acoes.trocarSecao('conexao')),
    ] }),
  ] })
}

function cabecalho(p) {
  const { ui, conexao, config, prefs, acoes, colunas } = p
  const status = STATUS[conexao] ?? STATUS['sem-config']
  const endereco = String(config.host ?? '').replace(/^https?:\/\//, '')
  const ativo = prefs?.service_enabled !== false
  return ui.Box({ key: 'cabecalho', justifyContent: 'space-between', children: [
    ui.Text({ key: 'cabecalho-status', children: [
      ui.Text({ key: 'cabecalho-ponto', color: status.cor, children: '● ' }),
      ui.Text({ key: 'cabecalho-texto', bold: true, children: status.texto }),
      ui.Text({ key: 'cabecalho-host', dimColor: true, children: ` · ${cortar(endereco, Math.max(8, colunas - 34))}` }),
    ] }),
    prefs
      ? ui.Button({ key: 'servico', plain: true, hover: lit('servico'), onPress: () => acoes.ajustar({ service_enabled: !ativo }), children: [
          ui.Text({ key: 'servico-ponto', color: ativo ? 'success' : 'warning', children: ativo ? '● ' : '◌ ' }),
          ui.Text({ key: 'servico-texto', dimColor: !ativo, children: ativo ? 'ativo' : 'em pausa' }),
        ] })
      : ui.Text({ key: 'servico-vazio', children: '' }),
  ] })
}

function abas(p, atual) {
  const { ui, acoes } = p
  return ui.Box({ key: 'abas', flexWrap: 'wrap', columnGap: 2, marginTop: 1, children: SECOES.map((secao) =>
    ui.Button({ key: `aba-${secao.id}`, plain: true, hover: lit(`aba-${secao.id}`), onPress: () => acoes.trocarSecao(secao.id), children: [
      ui.Text({
        key: `aba-${secao.id}-t`,
        color: secao.id === atual ? 'suggestion' : undefined,
        dimColor: secao.id !== atual,
        bold: secao.id === atual,
        underline: secao.id === atual,
        children: secao.rotulo,
      }),
    ] }),
  ) })
}

function aviso(p) {
  const { ui, aviso: atual, acoes } = p
  if (!atual) return undefined
  return ui.Box({ key: 'aviso', marginTop: 1, justifyContent: 'space-between', children: [
    ui.Text({ key: 'aviso-t', color: atual.tipo === 'erro' ? 'error' : 'success', wrap: 'wrap', children: atual.texto }),
    ui.Button({ key: 'aviso-fechar', plain: true, dimColor: true, hover: lit('aviso-fechar'), onPress: acoes.limparAviso, children: '✕' }),
  ] })
}

function linhaDoItem(item) {
  const estado = ESTADOS_DO_ITEM[item.state] ?? item.state
  const modo = item.mode === 'hold' ? ' · segura' : ''
  return `${estado}${modo}`
}

function secaoNarracao(p) {
  const { ui, fila, config, prefs, acoes, colunas } = p
  const corrente = fila?.current ?? null
  const lendo = String(fila?.reading_state ?? 'Idle')
  const pausado = lendo === 'Paused'
  const itens = Array.isArray(fila?.items) ? fila.items.slice(0, 9) : []
  const largura = Math.max(12, colunas - 8)

  const agora = corrente
    ? ui.Box({ key: 'agora', flexDirection: 'column', children: [
        ui.Box({ key: 'agora-linha', justifyContent: 'space-between', children: [
          ui.Text({ key: 'agora-texto', children: [
            ui.Text({ key: 'agora-seta', color: pausado ? 'warning' : 'success', children: pausado ? '❚❚ ' : '▶ ' }),
            ui.Text({ key: 'agora-previa', children: cortar(`${corrente.source}: ${corrente.preview}`, largura) }),
          ] }),
          ui.Button({ key: 'agora-cancelar', plain: true, dimColor: true, hover: lit('agora-cancelar', VERMELHO), onPress: () => acoes.cancelar(corrente.id), children: '✕' }),
        ] }),
        ui.Box({ key: 'transporte', columnGap: 2, marginTop: 1, children: [
          botao(ui, 'pausar', pausado ? 'retomar' : 'pausar', () => acoes.controle(pausado ? 'resume' : 'pause'), { hotkey: 'p', variant: 'primary' }),
          botao(ui, 'parar', 'parar', () => acoes.controle('stop'), { hotkey: 's' }),
          botao(ui, 'pular', 'pular', acoes.pular, { hotkey: 'n' }),
        ] }),
      ] })
    : ui.Text({ key: 'agora-vazio', dimColor: true, children: 'nada tocando.' })

  const pendentes = itens.length
    ? itens.map((item, indice) => ui.Box({ key: `item-${item.id}`, flexDirection: 'column', marginBottom: indice === itens.length - 1 ? 0 : 1, children: [
        ui.Text({ key: `item-linha-${item.id}`, children: [
          ui.Text({ key: `item-n-${item.id}`, dimColor: true, children: `${indice + 1}  ` }),
          ui.Text({ key: `item-estado-${item.id}`, color: COR_DO_ESTADO[item.state], children: linhaDoItem(item) }),
          ui.Text({ key: `item-previa-${item.id}`, children: ` · ${cortar(`${item.source}: ${item.preview}`, Math.max(12, colunas - 22))}` }),
        ] }),
        ui.Box({ key: `item-acoes-${item.id}`, paddingLeft: 3, columnGap: 2, children: [
          ui.Button({ key: `tocar-${item.id}`, plain: true, hotkey: String(indice + 1), hover: lit(`tocar-${item.id}`), onPress: () => acoes.tocar(item.id), children: ui.Text({ key: `tocar-${item.id}-t`, color: 'suggestion', children: '▶ tocar' }) }),
          ui.Button({ key: `cancelar-${item.id}`, plain: true, dimColor: true, hover: lit(`cancelar-${item.id}`, VERMELHO), onPress: () => acoes.cancelar(item.id), children: 'cancelar' }),
        ] }),
      ] }))
    : [ui.Text({ key: 'fila-vazia', dimColor: true, wrap: 'wrap', children: 'vazia: a próxima resposta do agente aparece aqui.' })]

  const nestaSessao = [
    titulo(ui, 'titulo-sessao', 'nesta sessão'),
    alternancia(ui, 'mod-auto', 'narrar cada resposta ao fim do turno', config.auto, () => acoes.gravarMod({ auto: !config.auto })),
    alternancia(ui, 'mod-modo', 'falar ao chegar ao topo (senão fica pronta)', config.mode === 'speak', () => acoes.gravarMod({ mode: config.mode === 'speak' ? 'hold' : 'speak' })),
    campo(ui, 'mod-label', 'rótulo na fila', config.label, (valor) => acoes.gravarMod({ label: valor.trim() || 'claude' })),
  ]

  const noVox = prefs
    ? [
        titulo(ui, 'titulo-vox-narracao', 'no Vox'),
        alternancia(ui, 'narration-enabled', 'aceitar narração de agentes', prefs.narration_enabled, () => acoes.ajustar({ narration_enabled: !prefs.narration_enabled })),
        alternancia(ui, 'narration-auto', 'outros agentes falam sozinho (pedido sem modo)', prefs.narration_auto_speak, () => acoes.ajustar({ narration_auto_speak: !prefs.narration_auto_speak })),
        (() => {
          const { opcoes, valor } = opcoesNumericas([3, 5, 10, 20, 30, 50], prefs.narration_queue_limit, (n) => `${n} itens`)
          return escolha(ui, 'narration-limit', 'fila de até', opcoes, valor, (escolhido) => acoes.ajustar({ narration_queue_limit: Number(escolhido) }))
        })(),
        campo(ui, 'narration-model', 'modelo da adaptação', prefs.narration_model, (valor) => acoes.ajustar({ narration_model: valor.trim() || null }), { placeholder: 'padrão do servidor' }),
      ]
    : [semVox(p, 'narracao-sem-vox')]

  return [
    titulo(ui, 'titulo-agora', 'agora'),
    agora,
    titulo(ui, 'titulo-fila', itens.length ? `fila · ${itens.length} esperando` : 'fila'),
    ...pendentes,
    ...nestaSessao,
    ...noVox,
  ]
}

function secaoLeitura(p) {
  const { ui, prefs, catalogo, acoes } = p
  if (!prefs) return [semVox(p, 'leitura-sem-vox')]
  const velocidade = opcoesNumericas([0.75, 0.9, 1, 1.1, 1.25, 1.4, 1.5, 1.75, 2], prefs.speed, vezes)
  const folga = opcoesNumericas([0, 0.05, 0.1, 0.2, 0.3, 0.5], prefs.prebuffer_ratio, porcento)
  const erroDeVozes = catalogo?.voices?.error
  return [
    titulo(ui, 'titulo-voz', 'voz'),
    escolha(ui, 'voice', 'voz', opcoesDeVoz(catalogo, prefs.voice), String(prefs.voice ?? ''), (valor) => acoes.ajustar({ voice: valor })),
    erroDeVozes ? dica(ui, 'voz-erro', `lista de vozes indisponível: ${cortar(erroDeVozes, 80)}`) : undefined,
    escolha(ui, 'speed', 'velocidade', velocidade.opcoes, velocidade.valor, (valor) => acoes.ajustar({ speed: Number(valor) })),
    escolha(ui, 'prebuffer', 'folga antes de tocar', folga.opcoes, folga.valor, (valor) => acoes.ajustar({ prebuffer_ratio: Number(valor) })),
    ui.Box({ key: 'amostra', marginTop: 1, children: [botao(ui, 'ouvir-amostra', 'ouvir amostra', acoes.ouvirAmostra, { hotkey: 'o' })] }),
    titulo(ui, 'titulo-texto', 'preparo do texto'),
    alternancia(ui, 'narrate-tables', 'explicar tabelas em vez de ler célula a célula', prefs.narrate_tables, () => acoes.ajustar({ narrate_tables: !prefs.narrate_tables })),
    alternancia(ui, 'normalize', 'corrigir ortografia antes de ler (+3 a 7 s)', prefs.normalize_before_reading, () => acoes.ajustar({ normalize_before_reading: !prefs.normalize_before_reading })),
    alternancia(ui, 'reader-window', 'abrir a janela de leitura guiada junto', prefs.open_reader_on_read, () => acoes.ajustar({ open_reader_on_read: !prefs.open_reader_on_read })),
  ].filter(Boolean)
}

const ROTULOS_DE_SAIDA = [
  { value: 'paste', label: 'colar no app em foco' },
  { value: 'clipboard', label: 'só copiar' },
]
const ROTULOS_DE_ENVIO = [
  { value: 'disabled', label: 'nunca' },
  { value: 'keyword', label: 'ao dizer a palavra-chave' },
  { value: 'auto', label: 'sempre' },
]
const ROTULOS_DE_TECLA = [
  { value: 'enter', label: 'Enter' },
  { value: 'shift-enter', label: 'Shift+Enter' },
]
const ROTULOS_DE_INTENSIDADE = [
  { value: '1', label: '1 · fiel ao que foi dito' },
  { value: '2', label: '2 · equilibrada' },
  { value: '3', label: '3 · reescreve à vontade' },
]

function secaoDitado(p) {
  const { ui, prefs, catalogo, acoes, colunas } = p
  if (!prefs) return [semVox(p, 'ditado-sem-vox')]
  const vocabulario = Array.isArray(prefs.vocabulary) ? prefs.vocabulary : []
  const envio = prefs.submit_mode ?? 'disabled'
  return [
    titulo(ui, 'titulo-captura', 'captura'),
    escolha(ui, 'input-device', 'microfone', opcoesDeMicrofone(catalogo, prefs.input_device), String(prefs.input_device ?? ''), (valor) => acoes.ajustar({ input_device: valor || null })),
    escolha(ui, 'transcription-model', 'transcrição', opcoesDeModelo(catalogo, prefs.transcription_model), String(prefs.transcription_model ?? ''), (valor) => acoes.ajustar({ transcription_model: valor })),
    alternancia(ui, 'live', 'reconhecer enquanto falo', prefs.live_transcription, () => acoes.ajustar({ live_transcription: !prefs.live_transcription })),
    prefs.live_transcription
      ? alternancia(ui, 'live-show', 'mostrar o texto ao vivo numa janelinha', prefs.show_live_transcription, () => acoes.ajustar({ show_live_transcription: !prefs.show_live_transcription }))
      : undefined,
    titulo(ui, 'titulo-entrega', 'entrega'),
    escolha(ui, 'output-action', 'o texto vai', ROTULOS_DE_SAIDA, String(prefs.output_action ?? 'paste'), (valor) => acoes.ajustar({ output_action: valor })),
    escolha(ui, 'submit-mode', 'enviar depois', ROTULOS_DE_ENVIO, envio, (valor) => acoes.ajustar({ submit_mode: valor })),
    envio === 'keyword'
      ? campo(ui, 'submit-keyword', 'palavra-chave', prefs.submit_keyword, (valor) => acoes.ajustar({ submit_keyword: valor }))
      : undefined,
    envio !== 'disabled'
      ? escolha(ui, 'submit-key', 'tecla de envio', ROTULOS_DE_TECLA, String(prefs.submit_key ?? 'enter'), (valor) => acoes.ajustar({ submit_key: valor }))
      : undefined,
    titulo(ui, 'titulo-reescrita', 'reescrita'),
    alternancia(ui, 'rewrite', 'reescrever o que ditei antes de entregar', prefs.stt_rewrite_enabled, () => acoes.ajustar({ stt_rewrite_enabled: !prefs.stt_rewrite_enabled })),
    ...(prefs.stt_rewrite_enabled
      ? [
          escolha(ui, 'rewrite-preset', 'molde', opcoesDePreset(catalogo, prefs.stt_rewrite_preset), String(prefs.stt_rewrite_preset ?? ''), (valor) => acoes.ajustar({ stt_rewrite_preset: valor })),
          escolha(ui, 'rewrite-intensity', 'intensidade', ROTULOS_DE_INTENSIDADE, String(prefs.stt_rewrite_intensity ?? 2), (valor) => acoes.ajustar({ stt_rewrite_intensity: Number(valor) })),
          campo(ui, 'rewrite-model', 'modelo', prefs.stt_rewrite_model, (valor) => acoes.ajustar({ stt_rewrite_model: valor.trim() || null }), { placeholder: 'padrão do servidor' }),
        ]
      : []),
    titulo(ui, 'titulo-vocabulario', `vocabulário · ${vocabulario.length}/50`),
    vocabulario.length
      ? ui.Box({ key: 'vocabulario', flexWrap: 'wrap', columnGap: 2, children: vocabulario.map((termo) =>
          ui.Button({ key: `termo-${termo}`, plain: true, hover: lit(`termo-${termo}`, VERMELHO), onPress: () => acoes.ajustar({ vocabulary: vocabulario.filter((item) => item !== termo) }), children: [
            ui.Text({ key: `termo-${termo}-t`, children: cortar(termo, Math.max(8, colunas - 8)) }),
            ui.Text({ key: `termo-${termo}-x`, dimColor: true, children: ' ×' }),
          ] }),
        ) })
      : dica(ui, 'vocabulario-vazio', 'nomes e termos que a transcrição deve acertar.'),
    ui.Input({ key: 'vocabulario-novo', label: '+ termo', placeholder: 'Tauri, Traefik…', submitLabel: 'adicionar', onSubmit: (valor) => {
      const termos = valor.split(',').map((termo) => termo.trim()).filter(Boolean)
      if (termos.length) acoes.ajustar({ vocabulary: [...vocabulario, ...termos] })
    } }),
    titulo(ui, 'titulo-instrucoes', 'instruções para a transcrição'),
    ui.Input({ key: 'custom-instructions', value: prefs.custom_instructions ?? '', rows: 3, placeholder: 'ex.: escreva siglas em maiúsculas', submitLabel: 'salvar', onSubmit: (valor) => acoes.ajustar({ custom_instructions: valor }) }),
  ].filter(Boolean)
}

function secaoAtalhos(p) {
  const { ui, prefs, catalogo, acoes } = p
  if (!prefs) return [semVox(p, 'atalhos-sem-vox')]
  const comandos = Array.isArray(catalogo?.commands) ? catalogo.commands : []
  if (!comandos.length) return [dica(ui, 'atalhos-vazio', 'carregando os comandos do Vox…')]
  const linhas = comandos.flatMap((comando) => [
    ui.Box({ key: `atalho-${comando.id}`, flexDirection: 'column', marginTop: 1, children: [
      ui.Text({ key: `atalho-${comando.id}-nome`, bold: true, children: comando.label }),
      comando.hint ? dica(ui, `atalho-${comando.id}-dica`, comando.hint) : undefined,
      ui.Input({
        key: `atalho-${comando.id}-campo`,
        label: 'tecla',
        value: comando.binding ?? '',
        placeholder: 'sem atalho',
        submitLabel: 'aplicar',
        onSubmit: (valor) => acoes.ajustar({ shortcuts: { [comando.id]: valor.trim() } }),
      }),
      comando.failure ? ui.Text({ key: `atalho-${comando.id}-falha`, color: 'warning', wrap: 'wrap', children: `não registrou: ${comando.failure}` }) : undefined,
    ].filter(Boolean) }),
  ])
  return [
    dica(ui, 'atalhos-ajuda', 'combinações como Ctrl+Alt+L. Vazio solta o comando. Vale na hora.'),
    ...linhas,
  ]
}

function secaoGeral(p) {
  const { ui, prefs, acoes } = p
  if (!prefs) return [semVox(p, 'geral-sem-vox')]
  const volume = opcoesNumericas([0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1], prefs.sounds_volume, porcento)
  return [
    titulo(ui, 'titulo-app', 'aplicativo'),
    alternancia(ui, 'service', 'Vox ativo (desligado solta os atalhos e a ponte)', prefs.service_enabled, () => acoes.ajustar({ service_enabled: !prefs.service_enabled })),
    alternancia(ui, 'autostart', 'iniciar com o Windows', prefs.start_with_windows, () => acoes.ajustar({ start_with_windows: !prefs.start_with_windows })),
    escolha(ui, 'accent', 'cor de destaque', comAtual(CORES_DE_DESTAQUE, prefs.theme_accent), String(prefs.theme_accent ?? ''), (valor) => acoes.ajustar({ theme_accent: valor })),
    titulo(ui, 'titulo-sons', 'sons'),
    alternancia(ui, 'sounds', 'avisos sonoros', prefs.sounds_enabled, () => acoes.ajustar({ sounds_enabled: !prefs.sounds_enabled })),
    ...(prefs.sounds_enabled
      ? [
          escolha(ui, 'volume', 'volume', volume.opcoes, volume.valor, (valor) => acoes.ajustar({ sounds_volume: Number(valor) })),
          ui.Box({ key: 'testar-som', marginTop: 1, children: [botao(ui, 'tocar-aviso', 'tocar aviso', acoes.tocarAviso)] }),
        ]
      : []),
  ]
}

function linhaDeInfo(ui, chave, rotulo, valor, colunas) {
  return ui.Text({ key: chave, children: [
    ui.Text({ key: `${chave}-r`, dimColor: true, children: `${rotulo}  ` }),
    ui.Text({ key: `${chave}-v`, children: cortar(valor, Math.max(10, colunas - rotulo.length - 6)) }),
  ] })
}

function secaoConexao(p) {
  const { ui, prefs, config, acoes, colunas, conexao } = p
  const ponte = prefs
    ? [
        titulo(ui, 'titulo-vox-info', 'no Vox (muda nas preferências do app)'),
        linhaDeInfo(ui, 'info-servidor', 'servidor', `${prefs.api_base_url}${prefs.api_auth ? ' · com login' : ''}`, colunas),
        linhaDeInfo(ui, 'info-ponte', 'ponte', `${prefs.bridge_enabled ? 'ligada' : 'desligada'} · porta ${prefs.bridge_port}${prefs.bridge_tailscale ? ' · também na tailnet' : ''}`, colunas),
        linhaDeInfo(ui, 'info-arquivo', 'arquivo', prefs.settings_path, colunas),
        ui.Box({ key: 'conexao-acoes', columnGap: 2, marginTop: 1, flexWrap: 'wrap', children: [
          botao(ui, 'abrir-preferencias', 'abrir preferências', acoes.abrirPreferencias),
          botao(ui, 'reiniciar', 'reiniciar Vox', acoes.reiniciar, { dimColor: true }),
        ] }),
      ]
    : [dica(ui, 'conexao-ajuda', conexao === 'token'
        ? 'o token não confere: copie de novo de Preferências → Extensão.'
        : 'abra o Vox, copie o token de Preferências → Extensão e cole abaixo.')]
  return [
    titulo(ui, 'titulo-mod', 'esta máquina'),
    campo(ui, 'mod-host', 'endereço', config.host, (valor) => acoes.gravarMod({ host: valor.trim().replace(/\/$/, '') || 'http://127.0.0.1:8765' })),
    ui.Input({ key: 'mod-token', label: `token ${mascararToken(config.token)}`, value: '', placeholder: 'colar novo token', submitLabel: 'salvar', onSubmit: (valor) => {
      if (valor.trim()) acoes.gravarMod({ token: valor.trim() })
    } }),
    ui.Box({ key: 'recarregar', marginTop: 1, children: [botao(ui, 'testar', 'testar conexão', acoes.recarregar, { hotkey: 'r' })] }),
    ...ponte,
  ]
}

const DESENHO_DA_SECAO = {
  narracao: secaoNarracao,
  leitura: secaoLeitura,
  ditado: secaoDitado,
  atalhos: secaoAtalhos,
  geral: secaoGeral,
  conexao: secaoConexao,
}

export function desenharWidget(p) {
  const { ui, fila, conexao, colunas } = p
  const corrente = fila?.current ?? null
  const esperando = Array.isArray(fila?.items) ? fila.items.length : 0
  const linha = corrente
    ? `▶ ${cortar(`${corrente.source}: ${corrente.preview}`, colunas - 4)}`
    : `fila: ${esperando} esperando`
  return ui.Box({ key: 'widget', children: [
    ui.Text({ key: 'widget-linha', color: conexao === 'ok' ? 'success' : 'subtle', children: `${conexao === 'ok' ? '●' : '○'} ${linha}` }),
  ] })
}

export function desenharPainel(p) {
  const { ui, colunas } = p
  const atual = secaoEfetiva(p.secao, p.conexao)
  const corpo = DESENHO_DA_SECAO[atual](p).filter(Boolean)
  return ui.Box({ key: 'raiz', flexDirection: 'column', paddingX: 1, paddingTop: 1, children: [
    cabecalho(p),
    abas(p, atual),
    linhaDivisoria(ui, 'sep-abas', colunas),
    aviso(p),
    ui.Box({ key: `corpo-${atual}`, flexDirection: 'column', children: corpo }),
  ].filter(Boolean) })
}
