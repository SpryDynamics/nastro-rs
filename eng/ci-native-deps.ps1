#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Makes sure the native tools needed to build whisper.cpp (via whisper-rs-sys) are present on a CI agent:
  CMake and a C/C++ toolchain to compile it, and libclang for bindgen.
.DESCRIPTION
  Hosted agents normally ship most of this, so it only installs what is missing. On Windows it also points
  LIBCLANG_PATH at the directory holding libclang.dll for the rest of the job.
#>
$ErrorActionPreference = 'Stop'

function Set-JobVariable([string]$name, [string]$value) {
    Write-Host "##vso[task.setvariable variable=$name]$value"
    Set-Item -Path "Env:$name" -Value $value
}

if ($IsWindows) {
    if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
        Write-Host 'CMake not found; installing.'
        choco install cmake --installargs 'ADD_CMAKE_TO_PATH=System' -y --no-progress
        if ($LASTEXITCODE -ne 0) { throw 'installing cmake failed' }
        Write-Host '##vso[task.prependpath]C:\Program Files\CMake\bin'
        $env:PATH = 'C:\Program Files\CMake\bin;' + $env:PATH
    }

    function Find-LibClang {
        $dirs = @($env:LIBCLANG_PATH, 'C:\Program Files\LLVM\bin', 'C:\Program Files (x86)\LLVM\bin',
                  'C:\LLVM\bin') | Where-Object { $_ }
        foreach ($d in $dirs) { if (Test-Path (Join-Path $d 'libclang.dll')) { return $d } }
        return $null
    }
    $dir = Find-LibClang
    if (-not $dir) {
        Write-Host 'libclang not found; installing LLVM.'
        choco install llvm -y --no-progress
        if ($LASTEXITCODE -ne 0) { throw 'installing LLVM failed' }
        $dir = Find-LibClang
    }
    if (-not $dir) { throw 'libclang.dll was not found even after installing LLVM' }
    Set-JobVariable 'LIBCLANG_PATH' $dir
    Write-Host "LIBCLANG_PATH=$dir"
}
elseif ($IsLinux) {
    sudo apt-get update
    if ($LASTEXITCODE -ne 0) { throw 'apt-get update failed' }
    sudo apt-get install -y cmake clang libclang-dev
    if ($LASTEXITCODE -ne 0) { throw 'installing cmake/clang/libclang-dev failed' }
}
else {
    throw 'unsupported build platform'
}

cmake --version | Select-Object -First 1
