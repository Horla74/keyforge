# Rebuild the deterministic letter icons using Windows' vector font renderer.
Add-Type -AssemblyName System.Drawing
$iconOutput = Join-Path $PSScriptRoot '../crates/keyforge-platform/assets'
[System.IO.Directory]::CreateDirectory($iconOutput) | Out-Null
foreach ($spec in @(@('running', 'K', '#16A34A'), @('paused', 'S', '#EF4444'))) {
    $frames = @()
    foreach ($size in @(16, 24, 32, 48, 64, 128, 256)) {
        $bitmap = [System.Drawing.Bitmap]::new($size, $size)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.Clear([System.Drawing.Color]::Transparent)
        $path = [System.Drawing.Drawing2D.GraphicsPath]::new()
        $font = [System.Drawing.FontFamily]::new('Arial')
        $path.AddString($spec[1], $font, 1, 100, [System.Drawing.PointF]::new(0, 0), [System.Drawing.StringFormat]::GenericTypographic)
        $bounds = $path.GetBounds()
        $scale = ($size * 0.82) / $bounds.Height
        $matrix = [System.Drawing.Drawing2D.Matrix]::new($scale, 0, 0, $scale, ($size - $bounds.Width * $scale) / 2 - $bounds.X * $scale, $size * 0.09 - $bounds.Y * $scale)
        $path.Transform($matrix)
        $outline = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(220, 255, 255, 255), [single]($size * 0.035))
        $outline.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
        $brush = [System.Drawing.SolidBrush]::new([System.Drawing.ColorTranslator]::FromHtml($spec[2]))
        $graphics.DrawPath($outline, $path)
        $graphics.FillPath($brush, $path)
        $stream = [System.IO.MemoryStream]::new()
        $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
        $frames += ,@($size, $stream.ToArray())
        if ($size -eq 256) { $bitmap.Save((Join-Path $iconOutput ($spec[0] + '.png')), [System.Drawing.Imaging.ImageFormat]::Png) }
        $stream.Dispose(); $brush.Dispose(); $outline.Dispose(); $matrix.Dispose()
        $font.Dispose(); $path.Dispose(); $graphics.Dispose(); $bitmap.Dispose()
    }
    $output = [System.IO.File]::Create((Join-Path $iconOutput ($spec[0] + '.ico')))
    $writer = [System.IO.BinaryWriter]::new($output)
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$frames.Count)
    $offset = 6 + 16 * $frames.Count
    foreach ($frame in $frames) {
        $dimension = if ($frame[0] -eq 256) { 0 } else { $frame[0] }
        $writer.Write([byte]$dimension); $writer.Write([byte]$dimension)
        $writer.Write([byte]0); $writer.Write([byte]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32)
        $writer.Write([uint32]$frame[1].Length); $writer.Write([uint32]$offset)
        $offset += $frame[1].Length
    }
    foreach ($frame in $frames) { $writer.Write([byte[]]$frame[1]) }
    $writer.Dispose()
}
