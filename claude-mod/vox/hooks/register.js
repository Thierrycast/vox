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
 * que precisa, a montagem/interpretação dos pedidos vive em `pedidos.js` e o
 * desenho do painel em `painel.js`, que recebe as ações já prontas.
 *
 * Configuração, por máquina (sobrevive a /clear e ao fechamento):
 *   /vox host <endereço>     ex.: http://127.0.0.1:8765 — ou o IP do Tailscale
 *   /vox token <valor>       o bridge_token das preferências do Vox
 *   /vox on | off            narra sozinho ao fim de cada turno
 *   /vox speak | hold        fala na hora, ou deixa pronto para dar play
 *   /vox label <nome>        o rótulo desta sessão na fila do Vox
 *   /vox status              o estado da conexão e da fila
 *   /vox                     abre o painel: fila, transporte e as preferências do Vox
 *   /vox ultima              narra a última resposta
 *   /vox <texto>             narra o texto dado
 */

import { atom, read, update } from 'claude-code'
import * as ponte from './pedidos.js'
import { desenharPainel, desenharWidget, FRASE_DE_AMOSTRA } from './painel.js'

const CHAVE_CONFIG = 'vox-config'
const PAINEL = 'vox-painel'
const INTERVALO_DA_FILA = 1500
const VOLTAS_POR_PREFERENCIAS = 4

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
const estadoConfig = atom({ plugin: 'vox', key: 'config' }, null)
const estadoPreferencias = atom({ plugin: 'vox', key: 'preferencias' }, null)
const estadoCatalogo = atom({ plugin: 'vox', key: 'catalogo' }, null)
const estadoSecao = atom({ plugin: 'vox', key: 'secao' }, null)
const estadoAviso = atom({ plugin: 'vox', key: 'aviso' }, null)

const comConfig = (salva) => ({ ...CONFIG_PADRAO, ...(salva || {}) })

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

let voltasDoRelogio = 0

export function register(on) {
  on('session.start', async ($, e, next) => {
    try {
      await $.command.register({
        name: 'vox',
        description: 'Painel do Vox: fila, transporte e preferências; ou narra o texto dado',
        argumentHint: '[painel|ultima|on|off|speak|hold|status|host|token|label|texto]',
      })
    } catch (erro) {
      // O nome já existe — outro mod de Vox nesta máquina. Não é motivo para
      // derrubar o resto do registro.
    }
    update($, estadoConfig, async () => comConfig(await $.store.get(CHAVE_CONFIG))).catch(() => {})

    $.clock.every(INTERVALO_DA_FILA, async () => {
      const aberto = (await $.ui.panes()).some((pane) => pane.id === PAINEL)
      if (!aberto) return
      const config = comConfig(await $.store.get(CHAVE_CONFIG))
      if (!config.token) {
        update($, estadoConexao, () => 'sem-config').catch(() => {})
        return
      }
      voltasDoRelogio += 1
      try {
        const pedidoFila = ponte.montarFila(config)
        const fila = await ponte.interpretar(await $.http.fetch(pedidoFila.url, pedidoFila.init), ponte.MENSAGENS.fila)
        update($, estadoFila, () => fila).catch(() => {})
        update($, estadoConexao, () => 'ok').catch(() => {})
        const semPreferencias = (await read($, estadoPreferencias)) === null
        if (semPreferencias || voltasDoRelogio % VOLTAS_POR_PREFERENCIAS === 0) {
          const pedidoPreferencias = ponte.montarPreferencias(config)
          const preferencias = await ponte.interpretar(await $.http.fetch(pedidoPreferencias.url, pedidoPreferencias.init), ponte.MENSAGENS.ajuste)
          update($, estadoPreferencias, () => preferencias).catch(() => {})
        }
        if ((await read($, estadoCatalogo)) === null) {
          const pedidoCatalogo = ponte.montarCatalogo(config)
          const catalogo = await ponte.interpretar(await $.http.fetch(pedidoCatalogo.url, pedidoCatalogo.init), ponte.MENSAGENS.ajuste)
          update($, estadoCatalogo, () => catalogo).catch(() => {})
        }
      } catch (erro) {
        update($, estadoFila, () => null).catch(() => {})
        update($, estadoConexao, () => gravarFalha(erro)).catch(() => {})
      }
    })
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

    const gravar = async (nova) => {
      await $.store.set(CHAVE_CONFIG, nova)
      update($, estadoConfig, () => nova).catch(() => {})
    }

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

    // O painel é o destino do comando seco: quem terminou de configurar digita
    // /vox e quer ver a fila, não decorar subcomandos.
    if (args === '' || primeira === 'painel' || primeira === 'panel') {
      update($, estadoConfig, () => config).catch(() => {})
      update($, estadoCatalogo, () => null).catch(() => {})
      const aberto = await $.ui.open({ id: PAINEL, title: 'Vox', focus: true, closeOnEscape: true })
      if (!aberto.isPlaced) {
        return { text: `o painel não abriu: ${aberto.reason ?? 'motivo desconhecido'}` }
      }
      if (!config.token) {
        update($, estadoConexao, () => 'sem-config').catch(() => {})
        return { text: 'painel do Vox aberto.', isTextDisplayOnly: true }
      }
      try {
        const pedidoFila = ponte.montarFila(config)
        const fila = await ponte.interpretar(await $.http.fetch(pedidoFila.url, pedidoFila.init), ponte.MENSAGENS.fila)
        update($, estadoFila, () => fila).catch(() => {})
        update($, estadoConexao, () => 'ok').catch(() => {})
        const pedidoPreferencias = ponte.montarPreferencias(config)
        const preferencias = await ponte.interpretar(await $.http.fetch(pedidoPreferencias.url, pedidoPreferencias.init), ponte.MENSAGENS.ajuste)
        update($, estadoPreferencias, () => preferencias).catch(() => {})
      } catch (erro) {
        update($, estadoConexao, () => gravarFalha(erro)).catch(() => {})
      }
      return { text: 'painel do Vox aberto.', isTextDisplayOnly: true }
    }

    switch (primeira) {
      case 'on':
      case 'off': {
        const nova = { ...config, auto: primeira === 'on' }
        await gravar(nova)
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
        await gravar({ ...config, mode: modo })
        return { text: modo === 'speak' ? 'novas respostas falam sozinho ao chegarem ao topo.' : 'novas respostas ficam prontas, esperando o play do painel.', isTextDisplayOnly: true }
      }
      case 'host': {
        if (!valor) return { text: `uso: /vox host <endereço>. Hoje: ${config.host}` }
        const host = valor.replace(/\/$/, '')
        await gravar({ ...config, host })
        return { text: `endereço do Vox: ${host}`, isTextDisplayOnly: true }
      }
      case 'token': {
        if (!valor) return { text: 'uso: /vox token <valor>. O valor está nas preferências do Vox, seção Extensão.' }
        const nova = { ...config, token: valor }
        await gravar(nova)
        const pedido = ponte.montarFila(nova)
        let estado = 'fora-do-ar'
        try {
          const resposta = await $.http.fetch(pedido.url, pedido.init)
          await ponte.interpretar(resposta, ponte.MENSAGENS.fila)
          estado = 'ok'
        } catch (erro) {
          estado = gravarFalha(erro)
        }
        update($, estadoConexao, () => estado).catch(() => {})
        const mensagem = {
          ok: 'token gravado — conectado ao Vox.',
          token: 'token gravado, mas não bate: copie de novo das preferências do Vox.',
          'fora-do-ar': 'token gravado, mas o Vox não respondeu — confira o endereço (/vox host).',
        }
        return { text: mensagem[estado] ?? 'token gravado.', isTextDisplayOnly: true }
      }
      case 'label': {
        if (!valor) return { text: `uso: /vox label <nome>. Hoje: ${config.label}` }
        await gravar({ ...config, label: valor })
        return { text: `rótulo desta sessão na fila: ${valor}`, isTextDisplayOnly: true }
      }
      case 'ultima':
      case 'last': {
        const ultima = await read($, ultimaResposta)
        if (!ultima.trim()) return { text: 'ainda não há resposta para narrar neste turno.' }
        return { text: await narrar(ultima), isTextDisplayOnly: true }
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
      default: {
        return { text: await narrar(args), isTextDisplayOnly: true }
      }
    }
  })

  on('ui.render', { component: 'Pane', requestId: PAINEL }, async ($, e) => {
    const ui = $.ui.resolve(e)
    const colunas = e.props.bodyColumns ?? 60
    const fila = await read($, estadoFila)
    const conexao = (await read($, estadoConexao)) ?? 'sem-config'
    const config = comConfig((await read($, estadoConfig)) ?? (await $.store.get(CHAVE_CONFIG)))

    if (e.props.placement === 'widget') {
      return desenharWidget({ ui, fila, conexao, colunas })
    }

    const prefs = conexao === 'ok' ? await read($, estadoPreferencias) : null
    const catalogo = await read($, estadoCatalogo)
    const secao = await read($, estadoSecao)
    const aviso = await read($, estadoAviso)

    const avisar = (tipo, texto) => {
      update($, estadoAviso, () => ({ tipo, texto })).catch(() => {})
      if (tipo !== 'ok') return
      $.clock.after(4000, () => {
        update($, estadoAviso, (atual) => (atual?.texto === texto ? null : atual)).catch(() => {})
      })
    }

    const enviar = async (pedido, mensagens) => {
      const resposta = await $.http.fetch(pedido.url, pedido.init)
      return ponte.interpretar(resposta, mensagens)
    }

    const atualizarFila = async (configDoVez) => {
      const novaFila = await enviar(ponte.montarFila(configDoVez), ponte.MENSAGENS.fila)
      update($, estadoFila, () => novaFila).catch(() => {})
    }

    const reconectar = async (configDoVez) => {
      try {
        await atualizarFila(configDoVez)
        update($, estadoConexao, () => 'ok').catch(() => {})
        const novas = await enviar(ponte.montarPreferencias(configDoVez), ponte.MENSAGENS.ajuste)
        update($, estadoPreferencias, () => novas).catch(() => {})
        const novoCatalogo = await enviar(ponte.montarCatalogo(configDoVez), ponte.MENSAGENS.ajuste)
        update($, estadoCatalogo, () => novoCatalogo).catch(() => {})
        avisar('ok', 'conectado ao Vox.')
      } catch (erro) {
        update($, estadoConexao, () => gravarFalha(erro)).catch(() => {})
        avisar('erro', erro.message)
      }
    }

    const comando = async (pedido, mensagens) => {
      try {
        await enviar(pedido, mensagens)
        await atualizarFila(config)
      } catch (erro) {
        avisar('erro', erro.message)
      }
    }

    const acoes = {
      controle: (acao) => comando(ponte.montarControle(config, acao), ponte.MENSAGENS.simples),
      pular: () => comando(ponte.montarPular(config), ponte.MENSAGENS.pular),
      tocar: (id) => comando(ponte.montarTocar(config, id), ponte.MENSAGENS.item),
      cancelar: (id) => comando(ponte.montarCancelar(config, id), ponte.MENSAGENS.item),
      ajustar: async (mudancas) => {
        try {
          const novas = await enviar(ponte.montarAjuste(config, mudancas), ponte.MENSAGENS.ajuste)
          update($, estadoPreferencias, () => novas).catch(() => {})
          update($, estadoAviso, () => null).catch(() => {})
          if ('shortcuts' in mudancas || 'service_enabled' in mudancas) {
            await new Promise((pronto) => { $.clock.after(400, pronto) })
            const novoCatalogo = await enviar(ponte.montarCatalogo(config), ponte.MENSAGENS.ajuste)
            update($, estadoCatalogo, () => novoCatalogo).catch(() => {})
          }
        } catch (erro) {
          avisar('erro', erro.message)
        }
      },
      gravarMod: async (mudancas) => {
        const nova = { ...config, ...mudancas }
        await $.store.set(CHAVE_CONFIG, nova)
        update($, estadoConfig, () => nova).catch(() => {})
        if (!('host' in mudancas) && !('token' in mudancas)) return
        if (!nova.token) {
          update($, estadoConexao, () => 'sem-config').catch(() => {})
          return
        }
        await reconectar(nova)
      },
      trocarSecao: (id) => {
        update($, estadoSecao, () => id).catch(() => {})
        update($, estadoAviso, (atual) => (atual?.tipo === 'ok' ? null : atual)).catch(() => {})
      },
      recarregar: async () => {
        if (!config.token) {
          avisar('erro', 'falta o token: cole abaixo o da seção Extensão das preferências do Vox.')
          return
        }
        await reconectar(config)
      },
      abrirPreferencias: () => comando(ponte.montarAbrirPreferencias(config), ponte.MENSAGENS.simples),
      tocarAviso: () => comando(ponte.montarAvisoSonoro(config), ponte.MENSAGENS.simples),
      ouvirAmostra: () => comando(ponte.montarLeitura(config, FRASE_DE_AMOSTRA), ponte.MENSAGENS.simples),
      reiniciar: async () => {
        try {
          await enviar(ponte.montarReiniciar(config), ponte.MENSAGENS.simples)
          update($, estadoPreferencias, () => null).catch(() => {})
          update($, estadoCatalogo, () => null).catch(() => {})
          avisar('ok', 'Vox reiniciando: o painel volta sozinho em alguns segundos.')
        } catch (erro) {
          avisar('erro', erro.message)
        }
      },
      limparAviso: () => {
        update($, estadoAviso, () => null).catch(() => {})
      },
    }

    return desenharPainel({ ui, colunas, fila, conexao, config, prefs, catalogo, secao, aviso, acoes })
  })
}
