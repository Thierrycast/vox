declare module 'claude-code' {
  interface PluginState {
    'vox': {
      /** A última resposta principal do agente, para o `/vox` sem argumentos. */
      lastAnswer: string
      /** O último retrato da fila de narração, para desenhar o painel. */
      fila: unknown
      /** O último estado de conexão com o Vox: "ok", "sem-config", "fora-do-ar". */
      conexao: string
    }
  }
}
