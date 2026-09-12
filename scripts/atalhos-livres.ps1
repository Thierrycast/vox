<#
    Diz quais combinacoes globais estao livres nesta maquina.

    Atalho global e recurso disputado: quem registra primeiro leva, e o Vox sobe
    depois do navegador e do que estiver na inicializacao. Uma combinacao tomada
    falha em silencio -- a tecla simplesmente nao faz nada, e a conclusao natural
    e que o app esta quebrado.

    Este script pergunta ao proprio Windows, com a mesma API que o Vox usa por
    baixo (`RegisterHotKey`): registra, anota se conseguiu, e desregistra na
    hora. Nao deixa nada preso e nao interfere em quem ja tinha a tecla.

    Uso:
        pwsh -File scripts\atalhos-livres.ps1
        pwsh -File scripts\atalhos-livres.ps1 -Teclas "J,B,M" -Modificadores "Ctrl+Alt"
#>
[CmdletBinding()]
param(
    [string]$Teclas = "A,B,C,D,E,F,G,H,I,J,K,L,M,N,O,P,Q,R,S,T,U,V,W,X,Y,Z",
    [string]$Modificadores = "Ctrl+Alt,Ctrl+Shift"
)

$ErrorActionPreference = "Stop"

Add-Type -Namespace Vox -Name Hotkey -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true)]
public static extern bool RegisterHotKey(System.IntPtr hWnd, int id, uint fsModifiers, uint vk);

[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true)]
public static extern bool UnregisterHotKey(System.IntPtr hWnd, int id);
'@

$MOD_ALT = 0x0001
$MOD_CONTROL = 0x0002
$MOD_SHIFT = 0x0004
# Sem isto, o teste dispararia a acao de quem ja tem a tecla enquanto mede.
$MOD_NOREPEAT = 0x4000

function Get-Modificador {
    param([string]$Texto)

    $valor = $MOD_NOREPEAT
    foreach ($parte in $Texto.Split("+")) {
        switch ($parte.Trim().ToLower()) {
            "ctrl"  { $valor = $valor -bor $MOD_CONTROL }
            "alt"   { $valor = $valor -bor $MOD_ALT }
            "shift" { $valor = $valor -bor $MOD_SHIFT }
            default { throw "modificador desconhecido: $parte" }
        }
    }
    return $valor
}

# No teclado ABNT2 o AltGr e Ctrl+Alt: registrar Ctrl+Alt+<letra> tira do teclado
# o caractere que aquela letra produziria. Estas produzem caractere de verdade.
$OcupadasPeloAltGr = @{
    "Q" = "/"; "W" = "?"; "E" = "EUR"; "R" = "R$"; "C" = "cruzeiro"; "5" = "20AC"
}

$identificador = 0
$resultados = @()

foreach ($combo in $Modificadores.Split(",")) {
    $combo = $combo.Trim()
    if (-not $combo) { continue }
    $bits = Get-Modificador $combo

    foreach ($tecla in $Teclas.Split(",")) {
        $tecla = $tecla.Trim().ToUpper()
        if (-not $tecla) { continue }

        $identificador += 1
        $codigo = [byte][char]$tecla

        $registrou = [Vox.Hotkey]::RegisterHotKey([System.IntPtr]::Zero, $identificador, $bits, $codigo)
        if ($registrou) {
            [void][Vox.Hotkey]::UnregisterHotKey([System.IntPtr]::Zero, $identificador)
        }

        $aviso = ""
        if ($combo -eq "Ctrl+Alt" -and $OcupadasPeloAltGr.ContainsKey($tecla)) {
            $aviso = "rouba $($OcupadasPeloAltGr[$tecla]) do ABNT2"
        }

        $resultados += [pscustomobject]@{
            Combinacao = "$combo+$tecla"
            Estado     = if ($registrou) { "livre" } else { "tomada" }
            Ressalva   = $aviso
        }
    }
}

$livres = $resultados | Where-Object { $_.Estado -eq "livre" -and -not $_.Ressalva }
$tomadas = $resultados | Where-Object { $_.Estado -eq "tomada" }

Write-Host ""
Write-Host "Tomadas por outro programa ($($tomadas.Count)):" -ForegroundColor Yellow
if ($tomadas) { Write-Host ("  " + (($tomadas.Combinacao) -join ", ")) } else { Write-Host "  nenhuma" }

Write-Host ""
Write-Host "Livres e sem ressalva ($($livres.Count)):" -ForegroundColor Green
Write-Host ("  " + (($livres.Combinacao) -join ", "))

$comRessalva = $resultados | Where-Object { $_.Ressalva }
if ($comRessalva) {
    Write-Host ""
    Write-Host "Livres, mas roubam uma tecla do teclado brasileiro:" -ForegroundColor DarkYellow
    foreach ($item in $comRessalva) {
        Write-Host ("  {0,-16} {1}" -f $item.Combinacao, $item.Ressalva)
    }
}

Write-Host ""
Write-Host "Medido agora, nesta maquina, com a mesma API que o Vox usa."
Write-Host "Um programa que subir depois pode tomar uma combinacao que aparece livre aqui."
