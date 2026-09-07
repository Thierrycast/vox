<#
    Cria o atalho "Vox (dev)" na area de trabalho e no menu Iniciar.

    O atalho nao aponta para o executavel: aponta para o launcher, que compila
    se o codigo mudou e so entao sobe o app. Assim o atalho nao envelhece junto
    com o build - ver scripts\vox-dev.ps1.

    Nada e escrito na inicializacao do Windows de proposito. Enquanto o projeto
    estiver em desenvolvimento, subir sozinho no boot significaria descobrir um
    bug novo antes de pedir por ele.

    Uso:
        pwsh -File scripts\instalar-atalho.ps1
        pwsh -File scripts\instalar-atalho.ps1 -Remove
#>
[CmdletBinding()]
param([switch]$Remove)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$vbsPath = Join-Path $PSScriptRoot "vox-dev.vbs"
$iconPath = Join-Path $repoRoot "src-tauri\icons\icon.ico"
$shortcutName = "Vox (dev).lnk"

$targets = @(
    (Join-Path ([Environment]::GetFolderPath("Desktop")) $shortcutName),
    (Join-Path ([Environment]::GetFolderPath("Programs")) $shortcutName)
)

if ($Remove) {
    foreach ($target in $targets) {
        if (Test-Path $target) { Remove-Item $target -Force; Write-Host "removido: $target" }
    }
    return
}

if (-not (Get-Command "pwsh.exe" -ErrorAction SilentlyContinue)) {
    throw "pwsh.exe nao encontrado no PATH. O atalho depende do PowerShell 7."
}
if (-not (Test-Path $vbsPath)) { throw "nao achei $vbsPath" }

$shell = New-Object -ComObject WScript.Shell

foreach ($target in $targets) {
    $shortcut = $shell.CreateShortcut($target)
    # O alvo e o wscript, e nao o pwsh, porque o wscript nao abre console.
    $shortcut.TargetPath = Join-Path $env:SystemRoot "System32\wscript.exe"
    $shortcut.Arguments = '"{0}"' -f $vbsPath
    $shortcut.WorkingDirectory = $repoRoot
    $shortcut.Description = "Sobe o Vox a partir do codigo atual (compila se preciso)"
    if (Test-Path $iconPath) { $shortcut.IconLocation = "$iconPath,0" }
    $shortcut.Save()
    Write-Host "atalho criado: $target"
}

[Runtime.InteropServices.Marshal]::ReleaseComObject($shell) | Out-Null

Write-Host ""
Write-Host "O Vox nao sobe sozinho no boot - e o atalho que decide quando ele existe."
Write-Host "Log do atalho: $env:APPDATA\vox\config\launcher.log"
