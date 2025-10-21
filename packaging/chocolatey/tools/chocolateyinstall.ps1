$ErrorActionPreference = 'Stop'
$toolsDir   = "$(Split-Path -parent $MyInvocation.MyCommand.Definition)"
$url64      = 'https://github.com/F1R3FLY-io/MeTTa-Compiler/releases/download/v0.1.0/mettatron-windows-x86_64.zip'

$packageArgs = @{
  packageName   = $env:ChocolateyPackageName
  unzipLocation = $toolsDir
  url64bit      = $url64
  softwareName  = 'mettatron*'
  checksum64    = 'REPLACE_WITH_ACTUAL_CHECKSUM'
  checksumType64= 'sha256'
}

Install-ChocolateyZipPackage @packageArgs
