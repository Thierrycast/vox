' Abre o launcher sem console nenhum.
'
' Um atalho que aponta para o pwsh pisca uma janela preta a cada clique, mesmo
' com -WindowStyle Hidden: quem cria o console e o proprio Windows, antes de o
' PowerShell ter chance de escondê-lo. O wscript nao cria console, e o segundo
' argumento do Run (0) diz que o processo filho tambem nao mostra janela.
'
' A janela de compilacao, quando precisa existir, e aberta pelo proprio
' launcher - e essa a gente quer ver.

Dim shell, pasta, comando

Set shell = CreateObject("WScript.Shell")
pasta = Left(WScript.ScriptFullName, InStrRev(WScript.ScriptFullName, "\") - 1)

comando = "pwsh.exe -NoProfile -NoLogo -ExecutionPolicy Bypass -File """ & pasta & "\vox-dev.ps1"" -Silent"

shell.Run comando, 0, False
