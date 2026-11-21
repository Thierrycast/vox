/** Tipos de `pedidos.js` para o teste em TypeScript. */

export interface Config {
  host: string
  token: string
  mode?: string
  label?: string
}

export interface Pedido {
  url: string
  init: {
    method?: string
    headers: Record<string, string>
    body?: string
  }
}

export interface Resposta {
  status: number
  ok: boolean
  text?: string
}

export function cabecalhos(token: string): Record<string, string>
export function montarNarracao(config: Config, texto: string): Pedido
export function montarFila(config: Config): Pedido
export function montarTocar(config: Config, id: number): Pedido
export function montarCancelar(config: Config, id: number): Pedido
export function montarPular(config: Config): Pedido
export function montarControle(config: Config, acao: string): Pedido
export function montarPreferencias(config: Config): Pedido
export function montarAjuste(config: Config, mudancas: Record<string, unknown>): Pedido
export function montarCatalogo(config: Config): Pedido
export function montarAbrirPreferencias(config: Config): Pedido
export function montarAvisoSonoro(config: Config): Pedido
export function montarReiniciar(config: Config): Pedido
export function montarLeitura(config: Config, texto: string): Pedido

export const MENSAGENS: {
  narrar: Record<number, string>
  fila: Record<number, string>
  item: Record<number, string>
  pular: Record<number, string>
  ajuste: Record<number, string>
  simples: Record<number, string>
}

export function interpretar(
  resposta: Resposta,
  mensagens: Record<number, string>,
): Promise<Record<string, unknown>>
