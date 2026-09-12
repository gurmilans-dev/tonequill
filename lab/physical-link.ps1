# Score separate recordings of the same reference transmission. No audio capture
# or playback is performed. Every successful trial must pass CRC and match TX.
param(
    [Parameter(Mandatory)][string]$Reference,
    [Parameter(Mandatory)][string]$RecordingsDirectory,
    [string]$OutputDirectory = ''
)
$ErrorActionPreference = 'Stop'
$referencePath = (Resolve-Path -LiteralPath $Reference).Path
if (-not (Test-Path -LiteralPath $RecordingsDirectory -PathType Container)) {
    throw 'RecordingsDirectory must be an existing folder containing WAV files, not a WAV file. From the repository root, use -RecordingsDirectory captures if your recording is captures/35cm.wav.'
}
$recordingsPath = (Resolve-Path -LiteralPath $RecordingsDirectory).Path
$repositoryPath = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $repositoryPath ('target/physical-' + [guid]::NewGuid().ToString('N'))
}
# Input and output paths both follow the caller's PowerShell location, even
# when the separate process working directory points to System32.
$resultsPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
# Require a fresh output directory so previous trials cannot contaminate results.
if (Test-Path -LiteralPath $resultsPath) { throw 'Choose a new output directory for this experiment' }
$recordings = @(Get-ChildItem -LiteralPath $recordingsPath -Filter '*.wav' -File |
    Where-Object { $_.FullName -ne $referencePath } | Sort-Object Name)
if (-not $recordings.Count) { throw 'No recording WAV files found' }
New-Item -ItemType Directory -Path $resultsPath | Out-Null
Push-Location $repositoryPath
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    $programPath = Join-Path $repositoryPath 'target/release/tonequill.exe'
    $referenceCsv = Join-Path $resultsPath 'reference-symbols.csv'
    & $programPath diagnose $referencePath $referencePath --limit 0 --symbols-csv $referenceCsv |
        Set-Content (Join-Path $resultsPath 'reference.txt')
    if ($LASTEXITCODE -ne 0) { throw 'Reference is not a CRC-valid packet' }
    $referenceBits = @(Import-Csv -LiteralPath $referenceCsv).Count
    $rows = @()
    $index = 0
    foreach ($recording in $recordings) {
        $index++
        $stem = '{0:D4}-{1}' -f $index, $recording.BaseName
        $symbolsPath = Join-Path $resultsPath "$stem.csv"
        & $programPath diagnose $referencePath $recording.FullName --limit 0 --symbols-csv $symbolsPath 2>&1 |
            Set-Content (Join-Path $resultsPath "$stem.txt")
        $trialExitCode = $LASTEXITCODE
        $symbols = if (Test-Path -LiteralPath $symbolsPath) { @(Import-Csv -LiteralPath $symbolsPath) } else { @() }
        $compared = @($symbols | Where-Object { $_.reference -ne '' })
        $errors = @($compared | Where-Object { $_.reference -ne $_.received }).Count
        $rows += [pscustomobject]@{
            recording = $recording.FullName
            crc_and_reference_valid = ($trialExitCode -eq 0)
            reference_bits = $referenceBits
            compared_bits = $compared.Count
            bit_errors = $errors
            missing_bits = [Math]::Max(0, $referenceBits - $compared.Count)
            ber = if ($compared.Count) { $errors / $compared.Count } else { '' }
            exit_code = $trialExitCode
        }
    }
    $rows | Export-Csv -NoTypeInformation -Path (Join-Path $resultsPath 'summary.csv')
    $validCount = @($rows | Where-Object crc_and_reference_valid).Count
    Write-Host "$validCount/$($rows.Count) CRC-valid, reference-matching recordings; PER=$(1 - $validCount / $rows.Count)"
    Write-Host "Results: $resultsPath"
} finally { Pop-Location }
