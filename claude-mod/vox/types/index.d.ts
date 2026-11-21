declare module 'claude-code' {
  interface PluginState {
    'vox': {
      /** A última resposta principal do agente, para o `/vox ultima`. */
      lastAnswer: string
      /** O último retrato da fila de narração, para desenhar o painel. */
      fila: unknown
      /** O último estado de conexão com o Vox: "ok", "token", "sem-config", "fora-do-ar". */
      conexao: string
      /** A config do mod nesta máquina: host, token, auto, modo e rótulo. */
      config: unknown
      /** As preferências do Vox como a ponte as expõe (`GET /settings`). */
      preferencias: unknown
      /** Vozes, microfones, moldes e comandos que o Vox oferece (`GET /settings/catalog`). */
      catalogo: unknown
      /** A aba aberta no painel. */
      secao: string | null
      /** A última mensagem de sucesso ou erro do painel. */
      aviso: unknown
    }
  }
}
