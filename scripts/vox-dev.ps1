<#
    Sobe o Vox a partir do codigo que esta no disco agora.

    O atalho aponta para este script, e nao para um executavel: um .lnk que
    aponta direto para um .exe congela a versao daquele dia, e a cada mudanca no
    projeto o atalho passaria a abrir um Vox velho sem avisar. Aqui o binario e
    reconstruido quando alguma fonte e mais nova que ele, entao clicar no atalho
    e sempre "sobe a ultima versao".

    Tambem encerra o que estiver rodando antes de subir. Dois processos do Vox
    ao mesmo tempo nao convivem: atalhos globais sao exclusivos do primeiro que
    registrar, e o segundo fica aberto sem responder a nada.

    Uso normal e pelo atalho. No terminal:
        pwsh -File scripts\vox-dev.ps1              # sobe (compila se precisar)
        pwsh -File scripts\vox-dev.ps1 -ForceBuild  # recompila mesmo em dia
#>
[CmdletBinding()]
param(
    # Vindo do atalho: nao ha console para mostrar nada, entao a compilacao
    # precisa abrir a propria janela para o erro nao sumir.
    [switch]$Silent,
    [switch]$ForceBuild,
    # Uso interno: e o que a janela visivel de compilacao executa.
    [switch]$BuildOnly
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $repoRoot "src-tauri\Cargo.toml"
$exePath = Join-Path $repoRoot "src-tauri\target\release\vox.exe"
$launcherLog = Join-Path $env:APPDATA "vox\config\launcher.log"

# O que, mudando, torna o binario velho. Ficam de fora `target` (saida) e o
# `.env` (nao entra na compilacao, e lido a cada partida).
$sourcePaths = @(
    "src",
    "assets",
    "src-tauri\src",
    "src-tauri\capabilities",
    "src-tauri\icons",
    "src-tauri\build.rs",
    "src-tauri\Cargo.toml",
    "src-tauri\Cargo.lock",
    "src-tauri\tauri.conf.json"
)

$LOG_LIMIT_BYTES = 256 * 1024

function Write-Log {
    param([string]$Message)

    $folder = Split-Path -Parent $launcherLog
    if (-not (Test-Path $folder)) { New-Item -ItemType Directory -Path $folder -Force | Out-Null }
    if ((Test-Path $launcherLog) -and (Get-Item $launcherLog).Length -gt $LOG_LIMIT_BYTES) {
        Remove-Item $launcherLog -Force
    }

    $line = "{0}  {1}" -f (Get-Date -Format "yyyy-MM-dd HH:mm:ss"), $Message
    Add-Content -Path $launcherLog -Value $line -Encoding UTF8
    if (-not $Silent) { Write-Host $line }
}

function Get-LatestSourceChange {
    $newest = [datetime]::MinValue

    foreach ($relative in $sourcePaths) {
        $full = Join-Path $repoRoot $relative
        if (-not (Test-Path $full)) { continue }

        $items = if (Test-Path $full -PathType Container) {
            Get-ChildItem -Path $full -Recurse -File -ErrorAction SilentlyContinue
        } else {
            Get-Item -Path $full
        }

        foreach ($item in $items) {
            if ($item.LastWriteTimeUtc -gt $newest) { $newest = $item.LastWriteTimeUtc }
        }
    }
    return $newest
}

function Invoke-CargoBuild {
    & cargo build --release --manifest-path $manifestPath
    return ($LASTEXITCODE -eq 0)
}

# As variaveis do .env viram ambiente do processo. Sem isto o Vox aberto pelo
# atalho nao enxergaria o que o Vox aberto pelo terminal enxerga, e a diferenca
# apareceria como um 401 vindo da API, longe da causa.
function Import-DotEnv {
    $dotEnv = Join-Path $repoRoot ".env"
    if (-not (Test-Path $dotEnv)) { return 0 }

    $count = 0
    foreach ($line in Get-Content -Path $dotEnv -Encoding UTF8) {
        $trimmed = $line.Trim()
        if (-not $trimmed -or $trimmed.StartsWith("#")) { continue }

        $separator = $trimmed.IndexOf("=")
        if ($separator -lt 1) { continue }

        $name = $trimmed.Substring(0, $separator).Trim()
        $value = $trimmed.Substring($separator + 1).Trim().Trim('"')
        [Environment]::SetEnvironmentVariable($name, $value, "Process")
        $count += 1
    }
    return $count
}

# Encerra o que estiver rodando. Devolve quantas instancias caiu.
function Stop-Vox {
    $running = @(Get-Process -Name "vox" -ErrorAction SilentlyContinue)
    if ($running.Count -eq 0) { return 0 }

    $running | Stop-Process -Force -ErrorAction SilentlyContinue
    $running | Wait-Process -Timeout 5 -ErrorAction SilentlyContinue
    return $running.Count
}

# ------------------------------------------------------------------ execucao

if ($BuildOnly) {
    # A janela de compilacao encerra o app de novo antes de chamar o cargo.
    #
    # Quem chamou ja tinha encerrado, mas entre aquele momento e este passaram-se
    # os minutos da fila do cargo — e qualquer coisa que suba o Vox nesse
    # intervalo tranca o .exe outra vez. O sintoma e cruel: dez minutos de
    # compilacao terminando em "Acesso negado" no ultimo passo, o da substituicao
    # do arquivo.
    $null = Stop-Vox

    Write-Host "Vox - compilando a versao mais recente. Isso pode demorar alguns minutos."
    Write-Host ""
    if (Invoke-CargoBuild) { exit 0 }

    Write-Host ""
    Write-Host "A compilacao falhou. A janela fica aberta para voce ler o erro acima."
    Read-Host "Enter para fechar"
    exit 1
}

# Uma partida por vez.
#
# Duas execucoes ao mesmo tempo — dois cliques no atalho, ou um clique enquanto a
# anterior ainda compila — se atropelam: a segunda encerra o app, mas a primeira
# sobe outro assim que termina, e aí a compilacao da segunda falha ao substituir
# um .exe que voltou a estar em uso. O mutex e do sistema todo, entao vale entre
# processos; quem chega depois avisa e sai, em vez de estragar o que ja estava
# em andamento.
$exclusividade = New-Object System.Threading.Mutex($false, "Global\vox-dev-launcher")
if (-not $exclusividade.WaitOne(0)) {
    Write-Log "ja ha uma partida em andamento; este clique foi ignorado"
    exit 0
}

try {

$needsBuild = $ForceBuild.IsPresent -or -not (Test-Path $exePath)
if (-not $needsBuild) {
    $needsBuild = (Get-LatestSourceChange) -gt (Get-Item $exePath).LastWriteTimeUtc
}

# Encerrar vem antes de compilar, e nao depois: o Windows tranca o .exe em
# execucao, e o cargo so descobre isso no fim, ao tentar substituir o arquivo.
# Compilar primeiro custava dez minutos para terminar em "Acesso negado".
$encerradas = Stop-Vox
if ($encerradas -gt 0) {
    Write-Log "encerrando $encerradas instancia(s) que ja estavam abertas"
}

if ($needsBuild) {
    Write-Log "codigo mais novo que o binario - recompilando"

    # A data do build e a do seu INICIO, e nao a do fim.
    #
    # O cargo le cada arquivo em algum momento entre um e outro; um arquivo
    # salvo durante a compilacao nao entra nela, mas ficaria com data anterior a
    # do binario e passaria por atualizado na proxima partida. O sintoma e o
    # pior possivel: o app sobe sem a mudanca e sem nada dizer que faltou algo.
    $inicioDaCompilacao = Get-Date

    $built = if ($Silent) {
        # A compilacao ganha janela propria: dois minutos de silencio absoluto
        # seriam indistinguiveis de um atalho quebrado.
        $arguments = @("-NoProfile", "-NoLogo", "-ExecutionPolicy", "Bypass",
                       "-File", $PSCommandPath, "-BuildOnly")
        (Start-Process -FilePath "pwsh.exe" -ArgumentList $arguments -PassThru -Wait).ExitCode -eq 0
    } else {
        Invoke-CargoBuild
    }

    if (-not $built) {
        # O binario anterior continua no disco - o cargo falhou antes de
        # substitui-lo. Subir a versao velha e melhor que deixar a maquina sem
        # Vox nenhum por causa de um erro de compilacao; a janela da compilacao
        # fica aberta com o erro, e o log registra que a versao nao e a atual.
        if (-not (Test-Path $exePath)) {
            Write-Log "compilacao falhou e nao ha binario anterior - o Vox nao subiu"
            exit 1
        }
        Write-Log "compilacao falhou - subindo o binario anterior"
    } else {
        # Salvar um arquivo sem mudar o conteudo deixa o cargo sem nada para
        # religar, e o binario continua com a data antiga. Sem esta linha,
        # aquele arquivo ficaria para sempre "mais novo que o build" e toda
        # partida abriria uma janela de compilacao para nao compilar nada.
        (Get-Item $exePath).LastWriteTime = $inicioDaCompilacao
        Write-Log "compilado"
    }
}

$imported = Import-DotEnv
Start-Process -FilePath $exePath -WorkingDirectory $repoRoot
Write-Log ("iniciado - binario de {0}, {1} variaveis do .env" -f (Get-Item $exePath).LastWriteTime, $imported)

}
finally {
    $exclusividade.ReleaseMutex()
    $exclusividade.Dispose()
}
