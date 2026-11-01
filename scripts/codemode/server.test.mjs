import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, readFile, symlink, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
const server = fileURLToPath(new URL('./server.mjs', import.meta.url));
async function start(t, access) {
  const root = await mkdtemp(join(tmpdir(), 'monocode-codemode-'));
  const config = join(root, 'config.json');
  await writeFile(config, JSON.stringify({ mcpServers: { fixture: { command: process.execPath, args: [fileURLToPath(new URL('./test-fixture.mjs', import.meta.url))] } } }));
  const client = new Client({ name: 'test', version: '1' });
  t.after(async () => { await client.close(); await rm(root, { recursive: true, force: true }); });
  await client.connect(new StdioClientTransport({ command: process.execPath, args: [server, '--cwd', root, '--access', access, '--config', config] }));
  const execute = code => client.callTool({ name: 'execute', arguments: { code } });
  return { root, client, execute };
}
test('read-only MCP batches reads, confines paths, and preserves process-local store', async t => {
  const { root, client, execute } = await start(t, 'read-only');
  await writeFile(join(root, 'a.txt'), 'hello');
  await symlink(tmpdir(), join(root, 'outside'));
  const found = await client.callTool({ name: 'search', arguments: {} });
  assert.match(JSON.stringify(found), /read/);
  assert.doesNotMatch(JSON.stringify(found), /tools\.shell|fixture__double|tools\.write/);
  const result = await execute('const [a,b] = await Promise.all([tools.read({path:"a.txt"}),tools.list({path:"."})]); store("value",a); return {a, count:b.entries.length};');
  assert.equal(result.isError, false); assert.match(JSON.stringify(result), /hello/);
  assert.match(JSON.stringify(await execute('return load("value");')), /hello/);
  assert.equal((await execute('return await tools.read({path:"outside/nope"});')).isError, true);
  assert.equal((await execute('return await tools.read({path:"../nope"});')).isError, true);
  assert.equal((await execute('return await tools.shell({command:"echo unsafe"});')).isError, true);
});
test('Full Access MCP bridges downstream tools and executes bounded commands', async t => {
  const { root, client, execute } = await start(t, 'full');
  const result = await execute('const n = await tools.fixture__double({value:21}); await tools.write({path:"result.txt",content:String(n.value)}); const s=await tools.shell({command:"printf SHELL_OK"}); return {n,s};');
  assert.equal(result.isError, false); assert.match(JSON.stringify(result), /SHELL_OK/);
  assert.equal(await readFile(join(root, 'result.txt'), 'utf8'), '42');
  const abort = new AbortController();
  const pending = client.callTool({ name: 'execute', arguments: { code: 'while(true) {}' } }, undefined, { signal: abort.signal });
  setTimeout(() => abort.abort(), 100);
  await assert.rejects(pending);
});
