# Plano de evolução do Vox

## Estado observado

- O HUD é uma `WebviewWindow` Tauri sem moldura. `shape_hud` em
  `src-tauri/src/dictation.rs` escolhia uma posição fixa a cada mudança de
  formato; isso apagava o deslocamento feito por arraste.
- A legenda guiada já recebe o plano de trechos e o índice/palavra corrente.
  Ela agora permite rolagem manual temporária e clique de trecho para seek.
- A tela de preferências é uma página única longa, com `select` e checkbox
  nativos. Ela não comporta bem preferências de rotina, ajustes avançados,
  providers e atalhos.
- Os atalhos globais são registrados apenas ao iniciar o app. Há dois hoje:
  ditado e leitura da seleção/clipboard.
- A transcrição final é feita por `SpeechApi`. Não há ainda contrato para
  provider LLM nem pós-processamento de texto.

## Entregas, em commits independentes

1. **Widget flutuante** — corrigir sombra, duplo clique/maximização, manter
   posição durante mudanças de forma, legenda rolável e seek por trecho.
   Commit: `6941789`.
2. **Posição persistente e spawn** — salvar posição por monitor, criar ação
   “repor posição” e seletor de posição inicial no painel. A posição manual é
   a autoridade; spawn só vale sem uma posição salva.
3. **Comandos e atalhos** — introduzir um catálogo de comandos (ditar,
   cancelar ditado, mostrar/ocultar widget, ler seleção, pausar/retomar,
   parar leitura). Cada comando terá atalho configurável, validação de formato,
   aviso de conflito e aplicação sem reiniciar quando o plugin permitir.
4. **Preferências como app desktop** — trocar a página longa por navegação
   lateral: Geral, Ditado, Leitura, Widget, Atalhos, IA e Integrações. Criar
   toggles próprios, select custom acessível, scrollbar escura discreta e
   estados de salvar/erro. Sem alterar preferências existentes por acidente.
5. **Pós-processamento por IA** — separar transcrição crua do texto final.
   Criar provider configurável e presets de transformação; o toggle só ativa
   quando houver provider válido. Nunca guardar segredo em log ou UI aberta.
   Os primeiros perfis: fiel (sem IA), fala limpa, prompt, prompt detalhado,
   empresarial e tradução. Nível de intervenção passa como instrução, não como
   regra implícita.

## Critérios de aceite principais

- Arrastar o widget e abrir/fechar legenda não muda sua âncora visual.
- Pausado: a legenda respeita a rolagem. Tocando: após 2,5s sem rolagem ela
  retorna suavemente à palavra em foco.
- Clique em trecho carregado da legenda inicia aquele trecho; trecho ainda em
  geração não aparenta aceitar um comando que não possa executar.
- O widget não pode maximizar, virar aba nem ter sombra cortada.
- Mudanças de UI têm teste de comportamento onde houver lógica Rust/JS e
  verificação visual manual antes do commit.

## Rodada de usabilidade de 2026-11-21

### Concluído

- Pausa entre título, categoria e parágrafo na leitura: `split_text` lê por
  linha (`src-tauri/src/api.rs`), com testes. Ponto só encerra frase seguido de
  espaço (corrige "0.2.3" virando três frases).
- Ajuste só-para-a-voz (`src-tauri/src/texto_falado.rs`): ponto final em trecho
  sem pontuação, "2,0s" para "2,0 segundos" (ms, s, min, h), "né, né, né" para
  um "né".
- Animação de espera do ditado em vaivém, esquerda para direita e volta
  (`src/css/hud.css`).
- Endereço do servidor não é mais embutido no binário: `DEFAULT_BASE_URL`
  removido, `.env.example`, placeholders do painel e do assistente e testes do
  `stt_stream` sem IP nem host do lab. Sem servidor, as chamadas dizem
  "nenhum servidor configurado" em vez de erro de URL relativa.

### Não feito, com motivo

- **Limpeza da leitura por modelo rápido** (ruído de Markdown, tabelas,
  gráficos, vícios de fala além do "né"). Precisa de um molde de leitura no
  `/text/rewrite` do servidor (fora deste repositório) e de decisão sobre o
  modelo e o custo de latência antes do primeiro som. O app já tem
  `normalize_before_reading` (desligado por padrão, 4 a 7 s) e `narrate_tables`.
  Proposta: molde `leitura` com modelo de baixa latência, ligado por chave nova
  em Leitura, com prazo máximo (se estourar, lê o original).
- **Confirmar que o `/text/prepare` preserva quebras de linha.** Não deu para
  medir sem a credencial do servidor; se ele achatar, a pausa por título só
  funciona depois de corrigido lá.
- **Posições fixas do widget** (canto X, sem arrastar): item novo, não bug.
- **"Piscar" ao abrir o widget fora da legenda:** sem reprodução; reabrir com o
  cenário exato se voltar.
- Repositório ainda contém IP do lab no histórico do git (não reescrito, por
  ordem).
