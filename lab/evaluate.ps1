# Reproduce the checked-in synthetic measurements with PowerShell 7.
param([string]$OutputDirectory = '')
$ErrorActionPreference = 'Stop'
$repositoryPath = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repositoryPath 'docs/measurements' }
# Resolve against the caller's PowerShell location, not the process working
# directory (which can still be System32 in an interactive terminal).
$measurementPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $measurementPath | Out-Null
Push-Location $repositoryPath
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    $programPath = Join-Path $repositoryPath 'target/release/tonequill.exe'
    function Invoke-Evaluation([string]$Name, [string[]]$ProgramArguments) {
        & $programPath evaluate @ProgramArguments | Set-Content (Join-Path $measurementPath "$Name.csv")
        if ($LASTEXITCODE -ne 0) { throw "Evaluation failed: $Name" }
    }
    cargo run --release -p tonequill-core --example baseline | Set-Content (Join-Path $measurementPath 'baseline-before.csv')
    if ($LASTEXITCODE -ne 0) { throw 'Baseline failed' }
    cargo run --release -p tonequill-core --example baseline -- --modern | Set-Content (Join-Path $measurementPath 'baseline-after.csv')
    if ($LASTEXITCODE -ne 0) { throw 'Modern comparison failed' }
    Invoke-Evaluation 'awgn' @('--trials', '200', '--snr-db', '15,5,0,-5,-10,-15,-20')
    Invoke-Evaluation 'awgn-full-symbols' @('--trials', '200', '--snr-db', '15,-5,-10,-15', '--full-symbols')
    $combinedArguments = @('--trials', '100', '--gain', '0.3', '--end-gain-ratio', '0.5',
        '--one-carrier-gain', '2', '--clock-ppm', '1500', '--echo-delay-samples', '137',
        '--echo-gain', '0.3', '--dc-offset', '0.08')
    Invoke-Evaluation 'combined-short' ($combinedArguments + @('--snr-db', '15,5,0,-5,-10'))
    Invoke-Evaluation 'combined-maximum' ($combinedArguments + @('--payload-bytes', '256', '--snr-db', '15,5,0'))
    $biasArguments = @('--trials', '100', '--snr-db', '15,5,0', '--one-carrier-gain', '4',
        '--echo-delay-samples', '137', '--echo-gain', '0.3')
    Invoke-Evaluation 'uncalibrated-bias' ($biasArguments + @('--no-calibration'))
    Invoke-Evaluation 'calibrated-bias' $biasArguments
    Invoke-Evaluation 'clock-plus-5000' @('--trials', '30', '--payload-bytes', '256', '--snr-db', '15,0', '--clock-ppm', '5000')
    Invoke-Evaluation 'clock-minus-5000' @('--trials', '30', '--payload-bytes', '256', '--snr-db', '15,0', '--clock-ppm', '-5000')
    Invoke-Evaluation 'long-echo' @('--trials', '100', '--snr-db', '15,5,0', '--one-carrier-gain', '2',
        '--echo-delay-samples', '384', '--echo-gain', '0.5')
    Write-Host "Measurements saved to $measurementPath"
} finally { Pop-Location }
