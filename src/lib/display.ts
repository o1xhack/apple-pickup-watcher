import colorNames from "../../crates/apw-core/data/color-names.json";

export function colorLabel(color: string): string {
  const chinese = (colorNames as Record<string, string>)[color];
  return chinese ? `${chinese} · ${color}` : color;
}

export function productLabel(title: string): string {
  const color = Object.keys(colorNames).sort((a, b) => b.length - a.length)
    .find(color => title === color || title.endsWith(` ${color}`));
  return color ? title.slice(0, -color.length) + colorLabel(color) : title;
}
