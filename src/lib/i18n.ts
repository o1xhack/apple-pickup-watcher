import dictionary from "../../web/translations.json";
export type Language = "zh" | "en";
let language: Language = "zh";
export const getLanguage = (): Language => language;
export function setUiLanguage(next: Language): void { language = next === "en" ? "en" : "zh"; }
const translations = dictionary as Record<string, string>;
const keys = Object.keys(translations).sort((a,b) => b.length-a.length);
const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const templates = keys.filter(key => /\{\d+\}/.test(key)).map(key => ({
  pattern: new RegExp("^" + key.split(/\{\d+\}/).map(escape).join("(.*?)") + "$"),
  value: translations[key]!,
}));
export function t(text: string): string {
  if (language !== "en") return text;
  if (translations[text]) return translations[text];
  for (const template of templates) {
    const match = template.pattern.exec(text);
    if (match) return template.value.replace(/\{(\d+)\}/g, (_, index: string) => t(match[Number(index)+1] ?? ""));
  }
  let value=text;
  for(const key of keys) if(!key.includes("{")) value=value.split(key).join(translations[key]!);
  return value;
}
