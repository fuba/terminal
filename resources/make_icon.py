from PIL import Image, ImageDraw

def make_icon(size):
    img = Image.new('RGBA', (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    pad = max(1, size // 16)
    bg_color = (12, 14, 20, 255)
    border_color = (60, 100, 180, 255)
    title_color = (40, 60, 110, 255)
    prompt_color = (90, 200, 110, 255)
    cursor_color = (220, 220, 220, 255)
    radius = max(2, size // 8)

    # Window frame
    d.rounded_rectangle([pad, pad, size - pad - 1, size - pad - 1],
                        radius=radius, fill=bg_color, outline=border_color,
                        width=max(1, size // 32))

    # Title bar
    title_h = max(2, size // 6)
    d.rounded_rectangle([pad, pad, size - pad - 1, pad + title_h],
                        radius=radius, fill=title_color)
    d.rectangle([pad, pad + title_h - radius, size - pad - 1, pad + title_h],
                fill=title_color)

    # Traffic light dots
    if size >= 32:
        dot_r = max(1, title_h // 4)
        dot_y = pad + title_h // 2
        for i, color in enumerate([(255, 95, 86), (255, 189, 46), (39, 201, 63)]):
            cx = pad + title_h // 2 + i * title_h
            d.ellipse([cx - dot_r, dot_y - dot_r, cx + dot_r, dot_y + dot_r],
                      fill=color)

    # ">" prompt + cursor block in body
    body_top = pad + title_h + max(1, size // 16)
    if size >= 16:
        char_size = max(2, (size - pad * 2) // 3)
        x0 = pad + max(2, size // 8)
        y0 = body_top + max(1, size // 16)

        # ">" chevron via two diagonal lines
        thick = max(1, size // 12)
        d.line([(x0, y0), (x0 + char_size // 2, y0 + char_size // 2)],
               fill=prompt_color, width=thick)
        d.line([(x0 + char_size // 2, y0 + char_size // 2),
                (x0, y0 + char_size)],
               fill=prompt_color, width=thick)

        # cursor (underscore block)
        cx0 = x0 + char_size // 2 + max(2, size // 10)
        cy0 = y0 + char_size - max(1, char_size // 4)
        cw2 = char_size // 2
        ch2 = max(1, char_size // 5)
        d.rectangle([cx0, cy0, cx0 + cw2, cy0 + ch2], fill=cursor_color)

    return img

sizes = [16, 32, 48, 64, 128, 256]
imgs = [make_icon(s) for s in sizes]
imgs[0].save(
    'C:/Users/ec/terminal/resources/app.ico',
    format='ICO',
    sizes=[(s, s) for s in sizes],
    append_images=imgs[1:],
)
print('Created app.ico with sizes', sizes)
