[CmdletBinding()]
param(
    [switch]$Clean,
    [switch]$Compile,
    [switch]$Web,
    [switch]$Desktop
)

# If no switches are provided, default to running both clean and compile.
if (-not ($PSBoundParameters.ContainsKey('Clean') -or $PSBoundParameters.ContainsKey('Compile'))) {
    $Clean = $true
    $Compile = $true
}

# If no specific targets are provided, default to both.
if (-not ($PSBoundParameters.ContainsKey('Web') -or $PSBoundParameters.ContainsKey('Desktop'))) {
    $Web = $true
    $Desktop = $true
}

# Check for required build tools
if (-not (Get-Command "cargo" -ErrorAction SilentlyContinue)) {
    Write-Host "Error: Rust (cargo) is not installed or not in your PATH." -ForegroundColor Red
    Write-Host "Please install Rust from https://rustup.rs/ and restart your terminal." -ForegroundColor Yellow
    exit 1
}
if (-not (Get-Command "npm" -ErrorAction SilentlyContinue)) {
    Write-Host "Error: Node.js (npm) is not installed or not in your PATH." -ForegroundColor Red
    Write-Host "Please install Node.js from https://nodejs.org/ and restart your terminal." -ForegroundColor Yellow
    exit 1
}

# Check for required Rust target for WebAssembly
if (Get-Command "rustup" -ErrorAction SilentlyContinue) {
    $installedTargets = rustup target list --installed
    if ($installedTargets -notmatch "wasm32-unknown-unknown") {
        Write-Host "Installing missing Rust target 'wasm32-unknown-unknown'..." -ForegroundColor Cyan
        rustup target add wasm32-unknown-unknown
    }
}

# Check for wasm-bindgen CLI tool
if (-not (Get-Command "wasm-bindgen" -ErrorAction SilentlyContinue)) {
    Write-Host "Installing missing 'wasm-bindgen-cli' (version 0.2.108)..." -ForegroundColor Cyan
    cargo install wasm-bindgen-cli --version 0.2.108
}

function Clean-Project {
    Write-Host "--- Cleaning project ---" -ForegroundColor Cyan

    # Clean Rust artifacts
    Write-Host "Running 'cargo clean'..."
    cargo clean

    # Clean web artifacts
    $webDir = ".\web"
    if (Test-Path $webDir) {
        Write-Host "Cleaning web artifacts in '$webDir'..."

        # Remove root node_modules
        $nodeModules = Join-Path $webDir "node_modules"
        if (Test-Path $nodeModules) {
            Write-Host "Removing '$nodeModules'..."
            Remove-Item -Recurse -Force $nodeModules
        }

        # Remove package-specific node_modules and dist folders
        $packagesDir = Join-Path $webDir "packages"
        if (Test-Path $packagesDir) {
            Get-ChildItem -Path $packagesDir -Directory | ForEach-Object {
                $packageNodeModules = Join-Path $_.FullName "node_modules"
                if (Test-Path $packageNodeModules) {
                    Write-Host "Removing '$packageNodeModules'..."
                    Remove-Item -Recurse -Force $packageNodeModules
                }

                $packageDist = Join-Path $_.FullName "dist"
                if (Test-Path $packageDist) {
                    Write-Host "Removing '$packageDist'..."
                    Remove-Item -Recurse -Force $packageDist
                }
            }
        }
    }

    Write-Host "--- Clean phase complete ---" -ForegroundColor Green
}

function Compile-Project {
    Write-Host "--- Compiling project ---" -ForegroundColor Cyan

    if ($Web) {
        # Compile web components (which includes the wasm build)
        if (Test-Path ".\web") {
            Write-Host "Building web components..."
            Push-Location ".\web"
            npm install
            npm run build
            Pop-Location
        }
    }

    if ($Desktop) {
        # Compile desktop application
        Write-Host "Building desktop application (release)..."
        cargo build --release --package ruffle_desktop
    }

    Write-Host "--- Compile phase complete ---" -ForegroundColor Green
}

if ($Clean) {
    Clean-Project
}

if ($Compile) {
    Compile-Project
}

Write-Host "Script finished." -ForegroundColor Green
