'use strict';
// Somente parsing dos traces; não inicia processos nem faz conexões.
const fs = require('node:fs');
const path = require('node:path');
const directory = process.argv[2];
const traces = {};
function nonLocal(address) {
  address = address.toLowerCase();
  if (address.startsWith('::ffff:')) {
    const mapped = address.slice(7);
    if (mapped.includes('.')) return nonLocal(mapped);
    const words = mapped.split(':').map(x => parseInt(x, 16));
    if (words.length === 2) return nonLocal(`${words[0] >> 8}.${words[0] & 255}.${words[1] >> 8}.${words[1] & 255}`);
  }
  if (address.includes(':')) {
    const halves = address.split('::');
    const left = halves[0].split(':').filter(Boolean);
    const right = (halves[1] || '').split(':').filter(Boolean);
    const words = halves.length === 2 ? [...left, ...Array(8 - left.length - right.length).fill('0'), ...right] : left;
    const numbers = words.map(word => parseInt(word, 16));
    return !(numbers.length === 8 && numbers.slice(0, 7).every(number => number === 0) && numbers[7] <= 1);
  }
  return address !== '0.0.0.0' && !address.startsWith('127.');
}
for (const label of ['daemon', 'ollama', 'llm-probe', 'setup']) {
  const files = fs.readdirSync(directory).filter(f => f.startsWith(`${label}.trace.`)).sort();
  const addresses = new Set();
  const outbound = [];
  for (const file of files) {
    const text = fs.readFileSync(path.join(directory, file), 'utf8');
    for (const match of text.matchAll(/inet_addr\("([^"]+)"\)|inet_pton\(AF_INET6, "([^"]+)"/g)) addresses.add(match[1] || match[2]);
    for (const line of text.split('\n')) {
      if (/(?:connect|sendto|sendmsg)\(/.test(line) && line.includes('sa_family=AF_INET')) outbound.push({trace:file,event:line});
    }
  }
  traces[label] = {files:files.length,addresses:[...addresses].sort(),non_loopback_addresses:[...addresses].sort().filter(nonLocal),outbound_ip_events:outbound};
}
console.log(JSON.stringify(traces, null, 2));
