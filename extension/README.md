# Vox — extensão de navegador

Lê o texto selecionado em voz alta e **destaca na própria página** o trecho que
está sendo falado. Sem abrir janela nenhuma, sem duplicar o texto.

## Instalar

1. Abra `chrome://extensions` e ligue o **Modo do desenvolvedor**.
2. **Carregar sem compactação** e aponte para esta pasta.
3. Abra as **opções** da extensão e cole o token.

O token está em `bridge_token`, dentro do `settings.json` do Vox — atalho pela
bandeja: *"Abrir o arquivo de preferências"*. Caminho direto:

```
C:\Users\<você>\AppData\Roaming\vox\config\settings.json
```

Clique em **Salvar e testar**. Se o Vox estiver aberto, a resposta é imediata.

## Usar

| Gesto | O que faz |
|---|---|
| Botão direito na seleção → **Ler com o Vox** | lê e destaca |
| `Alt+Shift+L` | o mesmo, sem tirar a mão do teclado |
| Clique no ícone da extensão | o mesmo |
| **Passar o mouse num parágrafo** → botão ▸ | lê daquele parágrafo, sem selecionar |
| O atalho global do Vox (`Ctrl+Alt+L`) | lê **e a página acompanha** |
| `Esc` | para a leitura e apaga o destaque |

## Por que a página escuta o atalho do Vox

O Vox é um aplicativo de desktop e a extensão é um cliente dele: a ponte é um
servidor, e quem fala é a extensão. **Não existe canal do app para o navegador.**
Quando a leitura começava pelo atalho global, a página não ficava sabendo de
nada, e a voz falava sem destaque nenhum.

A saída é a própria página ver a tecla. Ela tem foco quando o texto foi
selecionado ali, então o mesmo `Ctrl+Alt+L` que o Windows entrega ao Vox chega a
um ouvinte leve que vive em toda página. Ao vê-lo, ela pergunta ao Vox o que está
sendo lido (`GET /current`) e passa a acompanhar.

A combinação vem do `/health`, e não de uma cópia aqui: uma segunda lista
divergiria no primeiro ajuste feito no painel, e o sintoma seria a leitura
funcionando com o destaque parado.

O contador de **geração** distingue "a leitura de antes continua" de "outra
começou". Sem ele, a extensão pegaria os trechos da leitura anterior e tentaria
destacá-los numa página que já mudou.

## Como funciona

Há dois arquivos injetados, e a diferença entre eles é proposital. O `page.js`
vive em toda página e é quase nada: o botão do hover e o ouvinte do atalho, sem
DOM próprio até o mouse parar em algum lugar, sem rede e sem observador de
mutação. O `content.js`, que carrega a máquina de destaque, continua entrando
**sob demanda** — seria desperdício tê-lo parado em toda aba esperando um clique
que quase nunca vem.

O Vox abre um servidor em `127.0.0.1:8765`. A extensão manda o texto, recebe de
volta **os trechos já divididos pelo Vox**, e pergunta a posição da fala a cada
120 ms para mover o destaque.

Os trechos vêm do Vox de propósito. Dividir frases dos dois lados daria listas
diferentes na primeira abreviação ou reticência, e o destaque passaria a apontar
para a frase errada — um erro que só apareceria no meio de um texto longo.

O destaque usa a **CSS Custom Highlight API**, que pinta um trecho sem criar
nenhum elemento. A alternativa clássica — envolver o texto em `<mark>` — muda o
DOM de uma página que não é nossa: quebra layout, dispara observadores do site e
invalida referências que o JavaScript dele guardou. Aqui nada é criado, movido ou
alterado.

## Sobre a segurança

`127.0.0.1` **não é uma fronteira de confiança**. Qualquer página que você abrir
pode fazer `fetch` para lá, e o navegador bloqueia a *resposta* por CORS — mas o
pedido chega e o efeito acontece. Por isso a checagem é feita no servidor, antes
de agir, e são duas:

1. **`Origin` tem que ser `chrome-extension://`** em tudo que muda estado. Corta
   toda página web: o navegador anexa `Origin` sempre que o método não é
   GET/HEAD, então uma página **não consegue** fazer um POST sem ele.

   Os `GET` (saúde, progresso, trechos) aceitam origem ausente, porque o Chrome
   não a anexa num GET simples vindo de uma página de extensão — e era isso que
   fazia o "Salvar e testar" desta tela responder 403 no primeiro contato de quem
   acabava de instalar.
2. **Token no cabeçalho `X-Vox-Token`.** Corta outras extensões e qualquer
   programa da máquina que descubra a porta.

A superfície é curta de propósito: dá para pedir uma leitura, parar, e perguntar
onde a fala está. Nada lê a área de transferência, abre janela ou muda
preferência.

Para fechar a porta de vez: `bridge_enabled: false` no `settings.json`.

## Limites conhecidos

- **Só navegadores Chromium.** O Firefox usa outro esquema de origem para
  extensões e a checagem recusaria.
- **A seleção precisa continuar na página.** Se ela navegar ou o conteúdo for
  reescrito durante a leitura, os intervalos perdem a referência e o destaque
  para — a fala continua.
- **Trecho que não casa é pulado, não quebra.** Quando a página tem espaçamento
  que o Vox colapsou de forma diferente, aquele trecho fica sem destaque e os
  seguintes continuam alinhados.
- **`::highlight()` aceita poucas propriedades** — cor, fundo, sombra e
  decoração de texto. Nada de borda ou espaçamento: é pintura sobre o texto, e é
  justamente essa limitação que garante que o destaque nunca desloque o layout.
