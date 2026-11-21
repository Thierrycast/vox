# Vox — mod do Claude Code

Narra as respostas do Claude Code no [Vox](../../README.md) (ditado e leitura
por voz). O texto da resposta final vai pela ponte HTTP do Vox, é adaptado para
fala natural (markdown, tabelas e código viram narração — sem resumir) e sai
pelo mesmo player da leitura. Nada passa pelo contexto do agente e nada segura
o turno: o pedido é disparar-e-esquecer.

## Instalação em desenvolvimento

```powershell
cd claude-mod/vox
claude plugin validate .
claude plugin test .
claude --plugin-dir .        # sessão com o mod carregado
```

## Configuração (por máquina, guardada no `$.store`)

```
/vox host http://127.0.0.1:8765     # ou o IP do Tailscale, para agentes remotos/WSL
/vox token <bridge_token>           # preferências do Vox → seção Extensão
/vox on|off                         # narra sozinho ao fim de cada turno
/vox speak|hold                     # fala na hora, ou deixa pronto para dar play
/vox label <nome>                   # rótulo desta sessão na fila
/vox status                         # conexão e fila
/vox painel                         # painel com transporte e fila
/vox                                # narra a última resposta
/vox <texto>                        # narra o texto dado
```

O token circula só localmente (loopback ou tailnet) e fica em texto plano no
`$.store`, o mesmo status do `settings.json` do Vox — nunca em commit.

## Agentes fora do Windows

- **Windows nativo:** `http://127.0.0.1:<porta>`.
- **WSL modo mirrored:** `http://127.0.0.1:<porta>` também funciona.
- **WSL modo NAT e máquinas do homelab:** ligue "Ponte no IP do Tailscale" nas
  preferências do Vox (reinicia o app) e aponte `/vox host` para o IP da
  tailnet do Windows. O tráfego é cifrado pelo próprio Tailscale.

## O que fica no Vox, e o que fica aqui

A adaptação do texto (preset `narracao` da speech-api), a fila serial e a fala
moram no Vox. Este mod só captura a resposta (`turn.complete`), manda pela
ponte e desenha o painel — o contrato `/narrate` é agnóstico de agente, e um
driver para outro CLI no futuro é só isto de novo.
