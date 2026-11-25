// Lists the app's translatable strings and what each language is missing.
//   npm run i18n:check            report missing and unused keys
//   npm run i18n:check -- --keys  print every key (to start a new language)
// Keys are the English text given to t(), tr(), translate() or msg().
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const root = new URL('../src/', import.meta.url).pathname;
const dictDir = join(root, 'i18n');
const languages = ['es', 'fr', 'de', 'pt-BR', 'ru'];

const files = [];
const walk = (dir) => {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) walk(path);
    else if (/\.(ts|tsx)$/.test(name) && !path.startsWith(dictDir + '/')) files.push(path);
  }
};
walk(root);
files.push(join(dictDir, 'index.ts'));

const call = /\b(?:t|tr|translate|msg)\(\s*(?:'((?:\\.|[^'\\])*)'|"((?:\\.|[^"\\])*)")/g;
const keys = new Map();
for (const file of files) {
  const text = readFileSync(file, 'utf8');
  for (const m of text.matchAll(call)) {
    const raw = m[1] ?? m[2];
    const key = raw.replace(/\\(['"\\])/g, '$1');
    if (!keys.has(key)) keys.set(key, relative(root, file));
  }
}

if (process.argv.includes('--keys')) {
  console.log(JSON.stringify([...keys.keys()], null, 2));
  process.exit(0);
}

const placeholders = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(',');
let problems = 0;
for (const lang of languages) {
  const source = readFileSync(join(dictDir, `${lang}.ts`), 'utf8');
  const dict = JSON.parse(source.slice(source.indexOf('= {') + 2, source.lastIndexOf('}') + 1));
  const missing = [...keys.keys()].filter((k) => !(k in dict));
  const unused = Object.keys(dict).filter((k) => !keys.has(k));
  const broken = Object.entries(dict).filter(([k, v]) => keys.has(k) && placeholders(k) !== placeholders(v));
  console.log(`${lang}: ${keys.size - missing.length}/${keys.size} translated`);
  for (const k of missing) console.log(`  missing: ${JSON.stringify(k)} (${keys.get(k)})`);
  for (const k of unused) console.log(`  unused: ${JSON.stringify(k)}`);
  for (const [k] of broken) console.log(`  placeholders differ: ${JSON.stringify(k)}`);
  problems += broken.length;
}
// Missing words fall back to English, so only broken placeholders fail.
process.exit(problems ? 1 : 0);
