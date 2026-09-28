# Ferme TOUTES les instances Chrome lancées par Spectra (jamais le Chrome
# personnel de l'utilisateur) et nettoie les profils orphelins sur disque.
# Usage: powershell -File scripts\kill-all-spectra.ps1
$ErrorActionPreference = "SilentlyContinue"

Write-Host "Recherche des instances Chrome lancées par Spectra..."
$procs = Get-CimInstance Win32_Process -Filter "Name='chrome.exe'" |
    Where-Object { $_.CommandLine -like "*Spectra\profiles*" -and $_.CommandLine -like "*--remote-debugging-port*" }

if ($procs.Count -eq 0) {
    Write-Host "Aucune instance Chrome de Spectra trouvée."
} else {
    Write-Host "Fermeture de $($procs.Count) instance(s) Chrome (et leurs sous-process)..."
    foreach ($p in $procs) {
        taskkill /T /F /PID $p.ProcessId | Out-Null
        Write-Host "  - PID $($p.ProcessId) fermé"
    }
}

Write-Host "Recherche des serveurs spectra-server.exe..."
$serverProcs = Get-Process -Name "spectra-server" -ErrorAction SilentlyContinue
if ($serverProcs) {
    $serverProcs | Stop-Process -Force
    Write-Host "$($serverProcs.Count) process spectra-server.exe fermé(s)."
}

$profilesDir = "$env:LOCALAPPDATA\Spectra\profiles"
if (Test-Path $profilesDir) {
    Write-Host "Suppression des profils Chrome de test dans $profilesDir..."
    Remove-Item -Path $profilesDir -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "Profils supprimés."
}

Write-Host "Terminé."
