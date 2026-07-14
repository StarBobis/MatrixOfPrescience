function parseHexColor(value: string): [number, number, number] | null {
  const hex = value.trim().replace(/^#/, "");
  const normalized =
    hex.length === 3 || hex.length === 4
      ? hex
          .slice(0, 3)
          .split("")
          .map((character) => `${character}${character}`)
          .join("")
      : hex.slice(0, 6);

  if (normalized.length !== 6 || !/^[0-9a-f]{6}$/i.test(normalized)) {
    return null;
  }

  return [0, 2, 4].map((offset) => Number.parseInt(normalized.slice(offset, offset + 2), 16)) as [
    number,
    number,
    number,
  ];
}

function toLinearChannel(channel: number) {
  const value = channel / 255;
  return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

export function getReadableTextColor(background: string) {
  const rgb = parseHexColor(background);

  if (!rgb) {
    return "#ffffff";
  }

  const luminance =
    0.2126 * toLinearChannel(rgb[0]) +
    0.7152 * toLinearChannel(rgb[1]) +
    0.0722 * toLinearChannel(rgb[2]);

  return luminance > 0.179 ? "#000000" : "#ffffff";
}
