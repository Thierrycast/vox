/* Vox — mod do Claude Code.
 *
 * O que ele faz: quando um turno termina, a resposta final pode virar fala no
 * Vox (o app de ditado e leitura do Thierry, rodando no cliente Windows). O
 * mod é fino de propósito: captura a resposta, manda pela ponte HTTP do Vox e
 * desenha o painel. Toda a adaptação do texto e a fala ficam no lado do Vox,
 * que já tem o pipeline — nada passa pelo contexto do agente, e nada segura
 * o turno.
 *
 * Sobre a forma do código: a análise estática só aceita a API (`$.http.fetch`,
 * `$.store`, `$.ui`) chamada literalmente dentro dos hooks — nem a sessão `$`
 * pode ser passada a uma função daqui. Por isso cada hook inlina as chamadas
 * que precisa, e a montagem/interpretação dos pedidos vive em `pedidos.js`,
 * que não sabe que Claude Code existe.
 *
 * Configuração, por máquina (sobrevive a /clear e ao fechamento):
 *   /vox host <endereço>     ex.: http://127.0.0.1:8765 — ou o IP do Tailscale
 *   /vox token <valor>       o bridge_token das preferências do Vox
 *   /vox on | off            narra sozinho ao fim de cada turno
 *   /vox speak | hold        fala na hora, ou deixa pronto para dar play
 *   /vox label <nome>        o rótulo desta sessão na fila do Vox
 *   /vox status              o estado da conexão e da fila
 *   /vox painel              abre o painel de controles
 *   /vox                     narra a última resposta
 *   /vox <texto>             narra o texto dado
 */

import { atom, read, update } from 'claude-code'
import * as ponte from './pedidos.js'

const CHAVE_CONFIG = 'vox-config'
const PAINEL = 'vox-painel'

const CONFIG_PADRAO = {
  host: 'http://127.0.0.1:8765',
  token: '',
  auto: false,
  mode: 'speak',
  label: 'claude',
}

const ultimaResposta = atom({ plugin: 'vox', key: 'lastAnswer' }, '')
const estadoFila = atom({ plugin: 'vox', key: 'fila' }, null)
const estadoConexao = atom({ plugin: 'vox', key: 'conexao' }, 'sem-config')

const comConfig = (salva) => ({ ...CONFIG_PADRAO, ...(salva || {}) })

// As chaves são as que a ponte manda: kebab-case em português, como o Rust
// serializa o estado do item.
const ESTADOS_DO_ITEM = {
  pendente: 'na fila',
  adaptando: 'adaptando…',
  pronto: 'pronta',
  tocando: 'tocando',
}

function linhaDoItem(item) {
  const estado = ESTADOS_DO_ITEM[item.state] ?? item.state
  const modo = item.mode === 'hold' ? ' · segura' : ''
  return `${estado}${modo} · ${item.source}: ${item.preview}`
}

function gravarFalha(erro) {
  return /token não bate/.test(String(erro && erro.message)) ? 'token' : 'fora-do-ar'
}

let painelAberto = false
let relogioAtivo = false

export function register(on) {
  on('session.start', async ($, e, next) => {
    try {
      await $.command.register({
        name: 'vox',
        description: 'Narra no Vox a última resposta, ou o texto dado',
        argumentHint: '[on|off|speak|hold|status|painel|host|token|label|texto]',
      })
    } catch (erro) {
      // O nome já existe — outro mod de Vox nesta máquina. Não é motivo para
      // derrubar o resto do registro.
    }
    return next(e)
  })

  // A resposta final do turno. Disparar e esquecer: a narração não pode
  // segurar o turno que acabou de terminar — nem o próximo.
  on('turn.complete', async ($, e, next) => {
    if (e.agentId !== undefined || e.isAborted) return next(e)

    const texto = String(e.answer ?? '')
    if (!texto.trim()) return next(e)

    update($, ultimaResposta, () => texto).catch(() => {})

    const config = comConfig(await $.store.get(CHAVE_CONFIG))
    if (config.auto && config.token) {
      const pedido = ponte.montarNarracao(config, texto)
      $.http.fetch(pedido.url, pedido.init).catch(() => {})
    }
    return next(e)
  })

  on('command.run', { command: 'vox' }, async ($, e) => {
    const config = comConfig(await $.store.get(CHAVE_CONFIG))
    const args = String(e.args ?? '').trim()
    const [primeira, ...resto] = args.split(/\s+/)
    const valor = resto.join(' ')

    // Narra um texto — a última resposta, ou o que veio depois do comando.
    const narrar = async (texto) => {
      if (!config.token) {
        return 'sem token: rode /vox token <valor> com o bridge_token das preferências do Vox (seção Extensão).'
      }
      const pedido = ponte.montarNarracao(config, texto)
      try {
        const resposta = await $.http.fetch(pedido.url, pedido.init)
        const resultado = await ponte.interpretar(resposta, ponte.MENSAGENS.narrar)
        return resultado.position === 0
          ? 'narração entregue ao Vox.'
          : `enfileirada — ${resultado.position} na frente dela.`
      } catch (erro) {
        return `não narrou: ${erro.message}`
      }
    }

    if (args === '' || primeira === undefined) {
      const ultima = await read($, ultimaResposta)
      if (!ultima.trim()) return { text: 'ainda não há resposta para narrar neste turno.' }
      return { text: await narrar(ultima), isTextDisplayOnly: true }
    }

    switch (primeira) {
      case 'on':
      case 'off': {
        const nova = { ...config, auto: primeira === 'on' }
        await $.store.set(CHAVE_CONFIG, nova)
        if (nova.auto && !nova.token) {
          return { text: 'auto-narração ligada, mas falta o token: /vox token <valor> (bridge_token nas preferências do Vox, seção Extensão).' }
        }
        return { text: primeira === 'on' ? 'auto-narração ligada.' : 'auto-narração desligada.', isTextDisplayOnly: true }
      }
      case 'speak':
      case 'falar':
      case 'hold':
      case 'segurar': {
        const modo = (primeira === 'speak' || primeira === 'falar') ? 'speak' : 'hold'
        await $.store.set(CHAVE_CONFIG, { ...config, mode: modo })
        return { text: modo === 'speak' ? 'novas respostas falam sozinho ao chegarem ao topo.' : 'novas respostas ficam prontas, esperando o play do painel.', isTextDisplayOnly: true }
      }
      case 'host': {
        if (!valor) return { text: `uso: /vox host <endereço>. Hoje: ${config.host}` }
        const host = valor.replace(/\/$/, '')
        await $.store.set(CHAVE_CONFIG, { ...config, host })
        return { text: `endereço do Vox: ${host}`, isTextDisplayOnly: true }
      }
      case 'token': {
        if (!valor) return { text: 'uso: /vox token <valor>. O valor está nas preferências do Vox, seção Extensão.' }
        const nova = { ...config, token: valor }
        await $.store.set(CHAVE_CONFIG, nova)
        const pedido = ponte.montarFila(nova)
        let estado = 'fora-do-ar'
        try {
          const resposta = await $.http.fetch(pedido.url, pedido.init)
          await ponte.interpretar(resposta, ponte.MENSAGENS.fila)
          estado = 'ok'
        } catch (erro) {
          estado = gravarFalha(erro)
        }
        const mensagem = {
          ok: 'token gravado — conectado ao Vox.',
          token: 'token gravado, mas não bate: copie de novo das preferências do Vox.',
          'fora-do-ar': 'token gravado, mas o Vox não respondeu — confira o endereço (/vox host).',
        }
        return { text: mensagem[estado] ?? 'token gravado.', isTextDisplayOnly: true }
      }
      case 'label': {
        if (!valor) return { text: `uso: /vox label <nome>. Hoje: ${config.label}` }
        await $.store.set(CHAVE_CONFIG, { ...config, label: valor })
        return { text: `rótulo desta sessão na fila: ${valor}`, isTextDisplayOnly: true }
      }
      case 'status': {
        if (!config.token) {
          return { text: `sem token configurado. host: ${config.host} — /vox token <valor> para completar.` }
        }
        const pedido = ponte.montarFila(config)
        try {
          const resposta = await $.http.fetch(pedido.url, pedido.init)
          const fila = await ponte.interpretar(resposta, ponte.MENSAGENS.fila)
          const corrente = fila.current ? `tocando: ${linhaDoItem(fila.current)}` : 'nada tocando.'
          const naFila = fila.items?.length ?? 0
          return { text: `conectado. ${corrente} ${naFila} na fila. auto: ${config.auto ? 'ligado' : 'desligado'}, modo: ${config.mode}.` }
        } catch (erro) {
          return { text: `sem conexão: ${erro.message}` }
        }
      }
      case 'painel': {
        const aberto = await $.ui.open({ id: PAINEL, title: 'Vox', focus: true, closeOnEscape: true })
        if (!aberto.isPlaced) {
          return { text: `o painel não abriu: ${aberto.reason ?? 'motivo desconhecido'}` }
        }
        painelAberto = true

        // O painel reflete a fila enquanto está aberto. Escrever em $.state
        // redesenha sozinho; o relógio só busca enquanto houver painel.
        const pedido = ponte.montarFila(config)
        try {
          const resposta = await $.http.fetch(pedido.url, pedido.init)
          const fila = await ponte.interpretar(resposta, ponte.MENSAGENS.fila)
          update($, estadoFila, () => fila).catch(() => {})
          update($, estadoConexao, () => 'ok').catch(() => {})
        } catch (erro) {
          update($, estadoFila, () => null).catch(() => {})
          update($, estadoConexao, () => gravarFalha(erro)).catch(() => {})
        }

        if (!relogioAtivo) {
          relogioAtivo = true
          $.clock.every(1500, async () => {
            if (!painelAberto) return
            const configDoVez = comConfig(await $.store.get(CHAVE_CONFIG))
            if (!configDoVez.token) {
              update($, estadoConexao, () => 'sem-config').catch(() => {})
              return
            }
            const pedidoDoVez = ponte.montarFila(configDoVez)
            try {
              const resposta = await $.http.fetch(pedidoDoVez.url, pedidoDoVez.init)
              const fila = await ponte.interpretar(resposta, ponte.MENSAGENS.fila)
              update($, estadoFila, () => fila).catch(() => {})
              update($, estadoConexao, () => 'ok').catch(() => {})
            } catch (erro) {
              update($, estadoFila, () => null).catch(() => {})
              update($, estadoConexao, () => gravarFalha(erro)).catch(() => {})
            }
          })
        }
        return { text: 'painel do Vox aberto.', isTextDisplayOnly: true }
      }
      default: {
        return { text: await narrar(args), isTextDisplayOnly: true }
      }
    }
  })

  on('ui.render', { component: 'Pane' }, async ($, e, next) => {
    if (e.requestId !== PAINEL) return next(e)

    const { Box, Text, Button } = $.ui.resolve(e)
    const fila = await read($, estadoFila)
    const conexao = (await read($, estadoConexao)) ?? 'sem-config'

    const statusConexao =
      conexao === 'ok' ? 'conectado ao Vox' :
      conexao === 'token' ? 'token não bate — /vox token <valor>' :
      conexao === 'fora-do-ar' ? 'Vox fora do ar' :
      'sem token — /vox token <valor>'

    const config = comConfig(await $.store.get(CHAVE_CONFIG))

    // Cada botão manda o próprio pedido: sem `$` atravessando função nenhuma,
    // a chamada de rede fica literal aqui, onde o analisador aceita.
    const mandar = (montado) => {
      $.http.fetch(montado.url, montado.init).catch(() => {})
    }

    const corrente = fila?.current ? [Text({ key: 'corrente' }, `▶ ${linhaDoItem(fila.current)}`)] : []
    const pendentes = (fila?.items ?? []).map((item, posicao) =>
      Text({ key: `item-${item.id}` }, `${posicao + 1}. ${linhaDoItem(item)}`)
    )

    return Box({ key: 'raiz' }, [
      Text({ key: 'status' }, `Vox — ${statusConexao}`),
      Box({ key: 'transporte', flexDirection: 'row' }, [
        Button({
          key: 'pausar',
          label: 'Pausar/Retomar',
          hotkey: 'p',
          onPress: () => mandar(ponte.montarControle(config, String(fila?.reading_state ?? '') === 'Paused' ? 'resume' : 'pause')),
        }),
        Button({ key: 'parar', label: 'Parar', hotkey: 's', onPress: () => mandar(ponte.montarControle(config, 'stop')) }),
        Button({ key: 'pular', label: 'Pular', hotkey: 'n', onPress: () => mandar(ponte.montarPular(config)) }),
      ]),
      ...corrente,
      ...(pendentes.length ? pendentes : [Text({ key: 'vazia' }, 'fila vazia')]),
      Text({ key: 'dica' }, '/vox status e /vox on|off fora do painel.'),
    ])
  })
}
