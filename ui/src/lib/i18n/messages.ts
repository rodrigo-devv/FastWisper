import en from './en.json';
import ptBr from './pt-BR.json';

export type Language = 'en' | 'pt-BR';
export const messages: Record<Language, Record<string, string>> = { en, 'pt-BR': ptBr };
const segments = (text: string) => text.split(/\{\d+\}/g);
const templates = Object.entries(en).filter(([key]) => key.includes('{0}'))
  .map(([key, message]) => ({ key, parts: segments(key), message }))
  .sort((a, b) => b.parts.join('').length - a.parts.join('').length);

const nestedMessage = (key: string, index: number) => {
  if (key === "{0} = {1}: {2}") return index === 2;
  if (key === "{0} já é o atalho de {1}; escolha outro") return index === 1;
  if (['config inválida em {0}: {1}', 'valor inválido para {0}: {1}', 'falha ao acessar {0}: {1}'].includes(key)) return index === 1;
  if (index === 0 && ['Não foi salvo: {0}', 'Salvo, mas o FastWisper recusou a config: {0}', 'Não foi possível reiniciar: {0}', 'Não foi possível apagar: {0}', 'Não foi possível mudar a nuvem: {0}', 'Download falhou: {0}', 'O som não tocou: {0}', 'O teste não rodou: {0}'].includes(key)) return true;
  return index === 0 && ['llm.endpoint: {0}', 'Texto copiado: cole com Ctrl+V ({0}).', 'Texto colado, mas não enviado ({0}).', 'Ollama: {0}', 'Ollama: {0}.'].includes(key);
};

/** Não traduz parâmetros de conteúdo: títulos, texto de notas, IDs e caminhos. */
export function translate(language: Language, key: string, values: unknown[] = [], depth = 0): string {
  const message = messages[language][key];
  if (message !== undefined) {
    return message.replace(/\{(\d+)\}/g, (match, index) => index < values.length ? String(values[index]) : match);
  }
  // Erros do backend já chegam formatados. As mensagens versionadas fornecem
  // o molde; dados desconhecidos continuam intactos.
  if (language === 'en' && depth < 5 && /^[a-z_]+\.[a-z_.]+: /.test(key)) {
    const at = key.indexOf(': ');
    return `${key.slice(0, at)}: ${translate(language, key.slice(at + 2), [], depth + 1)}`;
  }
  if (language === 'en' && depth < 5) for (const { key: templateKey, parts, message } of templates) {
    if (!key.startsWith(parts[0])) continue;
    let rest = key.slice(parts[0].length);
    const parameters: string[] = [];
    let valid = true;
    for (let index = 1; index < parts.length; index++) {
      const part = parts[index];
      if (index === parts.length - 1) {
        if (!rest.endsWith(part)) { valid = false; break; }
        parameters.push(part ? rest.slice(0, -part.length) : rest);
      } else {
        const end = part ? rest.indexOf(part) : -1;
        if (end < 0) { valid = false; break; }
        parameters.push(rest.slice(0, end)); rest = rest.slice(end + part.length);
      }
    }
    if (valid) return message.replace(/\{(\d+)\}/g, (_, index) => nestedMessage(templateKey, Number(index)) ? translate(language, parameters[index] ?? '', [], depth + 1) : parameters[index] ?? '');
  }
  return key;
}
