"""Draw the app icon: a pixel Latin 'b', black on white.

The glyph is written as a character grid and scaled with nearest-neighbour
sampling, so every pixel stays square and crisp - no anti-aliasing, no blur, no
font dependency. Editing the grid is how the icon is edited.

Run with --preview to see the glyph in the terminal instead of writing a file.
"""
import sys

# A slab-serif Latin lowercase b: a stem with a top serif, a bowl on the lower
# right, and a bottom serif. Two pixels of stroke weight at this size.
GLYPH = [
    "...####.......",
    "....##........",
    "....##........",
    "....##........",
    "....##........",
    "....##........",
    "....##........",
    "....##........",
    "....#####.....",
    "...#######....",
    "..####..###...",
    ".###.....###..",
    ".###.....###..",
    "..####..###...",
    "...#######....",
    "..#######.....",
]

BLACK = (0, 0, 0)
WHITE = (255, 255, 255)


def check_grid(rows):
    widths = {len(row) for row in rows}
    if len(widths) != 1:
        raise SystemExit(f"the glyph rows are not all the same width: {sorted(widths)}")
    for row in rows:
        for character in row:
            if character not in ".#":
                raise SystemExit(f"unexpected character {character!r} in the glyph")
    return widths.pop()


def preview():
    print()
    for row in GLYPH:
        print("   " + row.replace(".", " ").replace("#", "█"))
    print()
    print(f"   grid: {len(GLYPH)} rows x {len(GLYPH[0])} columns")


def ink_bounds(rows):
    """The bounding box of the drawn pixels, so centring uses the ink.

    The grid carries a column of empty cells on the left, and centring on the
    grid rather than on the ink would push the letter off centre.
    """
    filled = [
        (row_index, column_index)
        for row_index, row in enumerate(rows)
        for column_index, character in enumerate(row)
        if character == "#"
    ]
    if not filled:
        raise SystemExit("the glyph has no pixels in it")
    rows_seen = [r for r, _ in filled]
    columns_seen = [c for _, c in filled]
    return min(rows_seen), max(rows_seen), min(columns_seen), max(columns_seen)


def render(path, size=128, margin=8):
    from PIL import Image

    width = check_grid(GLYPH)
    top, bottom, left, right = ink_bounds(GLYPH)
    trimmed = [row[left:right + 1] for row in GLYPH[top:bottom + 1]]
    # The largest whole-pixel scale that leaves the margin on both sides, so the
    # glyph is never resampled unevenly and never touches an edge.
    usable = size - 2 * margin
    scale = max(min(usable // len(trimmed[0]), usable // len(trimmed)), 1)
    glyph_width = len(trimmed[0]) * scale
    glyph_height = len(trimmed) * scale
    image = Image.new("RGB", (size, size), WHITE)
    pixels = image.load()
    offset_x = (size - glyph_width) // 2
    offset_y = (size - glyph_height) // 2
    for row_index, row in enumerate(trimmed):
        for column_index, character in enumerate(row):
            if character != "#":
                continue
            for dy in range(scale):
                for dx in range(scale):
                    x = offset_x + column_index * scale + dx
                    y = offset_y + row_index * scale + dy
                    if 0 <= x < size and 0 <= y < size:
                        pixels[x, y] = BLACK
    image.save(path, "PNG", optimize=True)
    return scale, offset_x, offset_y, glyph_width, glyph_height


if __name__ == "__main__":
    if "--preview" in sys.argv:
        preview()
    else:
        target = sys.argv[1] if len(sys.argv) > 1 else "assets/browsrl-128.png"
        scale, ox, oy, gw, gh = render(target)
        print(f"wrote {target}: glyph {gw}x{gh} at scale {scale}, offset ({ox},{oy})")
        preview()
