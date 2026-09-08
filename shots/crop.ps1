Add-Type -AssemblyName System.Drawing
$src = [System.Drawing.Image]::FromFile("D:\STool\shots\after_home.png")
$bmp = New-Object System.Drawing.Bitmap(140, 330)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
$g.DrawImage($src, (New-Object System.Drawing.Rectangle(0,0,140,330)), (New-Object System.Drawing.Rectangle(0,60,140,330)), [System.Drawing.GraphicsUnit]::Pixel)
$bmp.Save("D:\STool\shots\crop_sidebar.png")
$g.Dispose(); $bmp.Dispose(); $src.Dispose()
$src2 = [System.Drawing.Image]::FromFile("D:\STool\shots\after_settings.png")
$bmp2 = New-Object System.Drawing.Bitmap(500, 180)
$g2 = [System.Drawing.Graphics]::FromImage($bmp2)
$g2.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
$g2.DrawImage($src2, (New-Object System.Drawing.Rectangle(0,0,500,180)), (New-Object System.Drawing.Rectangle(140,85,500,180)), [System.Drawing.GraphicsUnit]::Pixel)
$bmp2.Save("D:\STool\shots\crop_settings.png")
$g2.Dispose(); $bmp2.Dispose(); $src2.Dispose()
Write-Output "crops saved"
