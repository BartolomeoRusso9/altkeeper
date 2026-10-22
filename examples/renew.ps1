# Rinnova i profili di provisioning dell'iPhone che scadono entro 3 giorni.
# Versione Windows di renew.sh: la lancia l'Utilita' di pianificazione (vedi README, sezione
# Windows). Data ed esito vanno in renew.log, nello stesso formato di renew.sh.
# Notifiche su ntfy (opzionali): rinnovi riusciti ed errori (al massimo uno ogni 24 ore, perche'
# il telefono puo' essere fuori casa). Il topic sta in notify.conf, su una riga:
#   NTFY_TOPIC=un-nome-lungo-e-casuale
#
# Mettilo nella stessa cartella di altkeeper.exe, rp-pairing.plist e account.json.

$Dir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $Dir

# IP:porta dell'iPhone (di solito la porta e' 49152). Vuoto = il telefono si cerca in rete via
# Bonjour. Se la variabile e' gia' impostata vale quella.
if (-not $env:ALTKEEPER_PHONE) { $env:ALTKEEPER_PHONE = '' }

$Topic = $null
$Conf = Join-Path $Dir 'notify.conf'
if (Test-Path $Conf) {
    foreach ($line in Get-Content $Conf) {
        if ($line -match '^\s*NTFY_TOPIC\s*=\s*"?([^"\s]+)"?\s*$') { $Topic = $Matches[1] }
    }
}

function Notifica([string]$Titolo, [string]$Messaggio, [int]$Priorita, [string]$Tag) {
    if (-not $Topic) { return }
    $json = @{
        topic    = $Topic
        title    = $Titolo
        message  = $Messaggio
        priority = $Priorita
        tags     = @($Tag)
    } | ConvertTo-Json -Compress
    try {
        Invoke-RestMethod -Uri 'https://ntfy.sh' -Method Post `
            -ContentType 'application/json; charset=utf-8' `
            -Body ([System.Text.Encoding]::UTF8.GetBytes($json)) `
            -TimeoutSec 20 | Out-Null
        Write-Host 'ntfy: ok'
    } catch {
        Write-Host "ntfy: $($_.Exception.Message)"
    }
}

$Exe = Join-Path $Dir 'altkeeper.exe'
$Out = @(& $Exe renew --min-days 3 2>&1 | ForEach-Object { "$_" })
$Rc = $LASTEXITCODE

$Quando = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
Add-Content -Path (Join-Path $Dir 'renew.log') -Encoding UTF8 -Value (
    @("=== $Quando") + $Out + @("esito: $Rc")
)

$Flag = Join-Path $Dir '.ultimo-errore'
if ($Rc -ne 0) {
    # Errore: una notifica ogni 24 ore al massimo.
    $Vecchio = (-not (Test-Path $Flag)) -or ((Get-Item $Flag).LastWriteTime -lt (Get-Date).AddMinutes(-1440))
    if ($Vecchio) {
        $Coda = (($Out | Select-Object -Last 3) -join "`n")
        if ($Coda.Length -gt 300) { $Coda = $Coda.Substring(0, 300) }
        Notifica 'altkeeper: rinnovo non riuscito' $Coda 4 'warning'
        Set-Content -Path $Flag -Value ''
    }
} else {
    Remove-Item $Flag -Force -ErrorAction SilentlyContinue
    $N = @($Out | Select-String -Pattern 'installed on the phone|installato sul telefono').Count
    if ($N -gt 0) {
        $Apps = @($Out |
            Select-String -Pattern 'downloading a new profile|scarico un profilo nuovo' |
            ForEach-Object { $_.Line -replace '^\s+([^:]+):.*', '$1' } |
            Sort-Object -Unique)
        Notifica 'altkeeper: app rinnovate' ($Apps -join ' ') 3 'white_check_mark'
    }
}
exit $Rc
