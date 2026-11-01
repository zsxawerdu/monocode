#!/usr/bin/env node
// Local MCP adapter for Pi's standalone runtime. stdout belongs to MCP only.
import { realpath, readFile, writeFile, readdir, stat } from 'node:fs/promises';
import { dirname, resolve, relative, isAbsolute, join } from 'node:path';
import { homedir } from 'node:os';
import { spawn } from 'node:child_process';
import { CodemodeSandbox, renderToolSample } from '@earendil-works/pi-codemode';
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { ListToolsRequestSchema, CallToolRequestSchema } from '@modelcontextprotocol/sdk/types.js';

const args = process.argv.slice(2);
function option(name, fallback) {
  const index = args.indexOf(name);
  return index < 0 ? fallback : args[index + 1];
}
const root = await realpath(option('--cwd', process.cwd()));
const full = option('--access', 'read-only') === 'full';
const configPath = option('--config', join(homedir(), '.config', 'monocode', 'codemode.json'));
const MAX_DATA = 1024 * 1024;
const MAX_OUTPUT = 40_000;
const clients = [];
const tools = [];
const schema = (properties, required = []) => ({ type: 'object', properties, required, additionalProperties: false });
const string = { type: 'string' };
function required(value, name) {
  if (typeof value !== 'string' || !value.length) throw new Error(`${name} must be a nonempty string`);
  return value;
}
function inside(path) {
  const rel = relative(root, path);
  if (rel === '..' || rel.startsWith('../') || rel.startsWith('..\\') || isAbsolute(rel)) throw new Error('Path is outside this workspace');
  return path;
}
async function workspacePath(input, writing = false) {
  const path = inside(resolve(root, required(input, 'path')));
  try { return inside(await realpath(path)); }
  catch (error) {
    if (!writing || error.code !== 'ENOENT') throw error;
    // Do not follow a parent symlink outside the workspace when creating files.
    inside(await realpath(dirname(path)));
    return path;
  }
}
function command(binary, argv, signal) {
  return new Promise((resolveResult, reject) => {
    if (signal.aborted) return reject(new Error('Cancelled'));
    const child = spawn(binary, argv, { cwd: root, env: process.env, stdio: ['ignore', 'pipe', 'pipe'], detached: process.platform !== 'win32' });
    let output = '', stderr = '', bytes = 0, failure;
    const kill = () => {
      try { process.platform === 'win32' ? child.kill('SIGKILL') : process.kill(-child.pid, 'SIGKILL'); } catch {}
    };
    const abort = () => { failure = new Error('Cancelled'); kill(); };
    signal.addEventListener('abort', abort, { once: true });
    const timer = setTimeout(() => { failure = new Error('Command timed out'); kill(); }, 30_000);
    const collect = (data, error) => {
      bytes += data.length;
      if (bytes > MAX_DATA) { failure = new Error('Command output exceeds 1 MiB; narrow the request'); kill(); return; }
      if (error) stderr += data.toString(); else output += data.toString();
    };
    child.stdout.on('data', data => collect(data, false));
    child.stderr.on('data', data => collect(data, true));
    child.on('error', error => { failure = error; });
    child.on('close', code => {
      clearTimeout(timer); signal.removeEventListener('abort', abort);
      failure ? reject(failure) : resolveResult({ output, stderr, exit_code: code });
    });
  });
}
function add(name, description, inputSchema, execute) { tools.push({ name, description, inputSchema, execute }); }
add('read', 'Read UTF-8 text inside this workspace (maximum 1 MiB).', schema({ path: string }, ['path']), async ({ path }, { signal }) => {
  const target = await workspacePath(path);
  if ((await stat(target)).size > MAX_DATA) throw new Error('File exceeds 1 MiB');
  return readFile(target, { encoding: 'utf8', signal });
});
add('list', 'List one directory inside this workspace, up to 1000 entries.', schema({ path: string }), async ({ path = '.' }) => {
  const entries = await readdir(await workspacePath(path), { withFileTypes: true });
  return { entries: entries.slice(0, 1000).map(e => ({ name: e.name, directory: e.isDirectory(), symlink: e.isSymbolicLink() })), truncated: entries.length > 1000 };
});
add('search', 'Search workspace text using ripgrep. Respects ignore files, does not follow symlinks. Returns output, stderr, exit_code; 1 means no matches.', schema({ pattern: string, path: string }, ['pattern']), async ({ pattern, path = '.' }, { signal }) => {
  const target = await workspacePath(path);
  return command('rg', ['--no-config', '--line-number', '--color=never', '--max-count', '100', '--max-filesize', '1M', '--regexp', required(pattern, 'pattern'), '--', target], signal);
});
if (full) {
  add('write', 'Write UTF-8 text inside this workspace. Full Access only. Parent directory must exist.', schema({ path: string, content: string }, ['path', 'content']), async ({ path, content }, { signal }) => {
    if (typeof content !== 'string' || Buffer.byteLength(content) > MAX_DATA) throw new Error('Content must be text of at most 1 MiB');
    await writeFile(await workspacePath(path, true), content, { signal });
    return { written: path };
  });
  add('shell', 'Run a shell command in the workspace with Full Access. Use jj for VCS in .jj workspaces. Returns output, stderr, exit_code. Commands have a 30-second limit.', schema({ command: string }, ['command']), async (input, { signal }) => {
    const script = required(input.command, 'command');
    return process.platform === 'win32'
      ? command('cmd.exe', ['/d', '/s', '/c', script], signal)
      : command('/bin/sh', ['-c', script], signal);
  });
}
function envValues(values = {}) {
  return Object.fromEntries(Object.entries(values).map(([key, value]) => {
    if (typeof value !== 'string') throw new Error(`Expected a string for ${key}`);
    if (value.startsWith('env:')) {
      const resolved = process.env[value.slice(4)];
      if (resolved === undefined) throw new Error(`Missing environment variable ${value.slice(4)}`);
      return [key, resolved];
    }
    return [key, value];
  }));
}
const connectionErrors = [];
// User-level configuration only: a repository cannot automatically launch servers.
if (full) {
  let config = {};
  try { config = JSON.parse(await readFile(configPath, 'utf8')); }
  catch (error) { if (error.code !== 'ENOENT') throw new Error(`Cannot load codemode configuration: ${error.message}`); }
  for (const [name, definition] of Object.entries(config.mcpServers ?? {})) {
    if (definition.disabled) continue;
    if (!/^[a-zA-Z0-9_]+$/.test(name) || name === 'monocode_codemode') throw new Error('Downstream server names must contain only letters, digits, and underscores; recursion is not allowed');
    const client = new Client({ name: 'monocode-codemode', version: '0.1.0' });
    const transport = definition.url
      ? new StreamableHTTPClientTransport(new URL(definition.url), { requestInit: { headers: envValues(definition.headers) } })
      : new StdioClientTransport({ command: required(definition.command, 'command'), args: definition.args ?? [], cwd: root, env: { ...process.env, ...envValues(definition.env) }, stderr: 'inherit' });
    clients.push(client);
    try {
      await client.connect(transport, { timeout: 10_000 });
      let cursor;
      do {
        const page = await client.listTools(cursor ? { cursor } : undefined, { timeout: 10_000 });
        for (const tool of page.tools) {
          const exposed = `${name}__${tool.name.replace(/[^a-zA-Z0-9_]/g, '_')}`;
          if (tools.some(t => t.name === exposed)) throw new Error(`Tool name collision: ${exposed}`);
          add(exposed, tool.description ?? tool.name, tool.inputSchema, async (input, { signal }) => {
            const result = await client.callTool({ name: tool.name, arguments: input }, undefined, { signal, timeout: 30_000 });
            if (result.isError) throw new Error(JSON.stringify(result.content));
            return result.structuredContent ?? result;
          });
        }
        cursor = page.nextCursor;
      } while (cursor);
    } catch {
      // Errors can contain URLs/headers; expose only the server name.
      connectionErrors.push(`Could not connect downstream server: ${name}`);
      await client.close().catch(() => {});
    }
  }
}
let activeWorkers = 0;
let calls = 0;
const sandbox = new CodemodeSandbox({
  tools: tools.map(tool => ({ ...tool, execute: (input, context) => {
    if (++calls > 100) throw new Error('Maximum 100 tool calls per script');
    if (!input || typeof input !== 'object' || Array.isArray(input)) throw new Error('Tool arguments must be an object');
    return tool.execute(input, context);
  } })),
  timeoutMs: 60_000,
  memoryLimitBytes: 64 * 1024 * 1024,
  acquireWorker() {
    if (activeWorkers) throw new Error('A previous worker is still stopping; retry shortly');
    activeWorkers++;
    return () => { activeWorkers--; };
  },
});
let store = Object.create(null);
let executing = false;
const server = new Server({ name: 'monocode-codemode', version: '0.1.0' }, {
  capabilities: { tools: {} },
  instructions: 'Use search to discover tools, then execute JavaScript to batch independent calls and filter results. Scripts support tools.NAME(args), ALL_TOOLS, text(), image(), store(), load(), and top-level await/return. Only explicit output returns to your context. Tool results stay inside the script. This server has its own tools; host-agent tools are not implicitly available.',
});
server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: [
  { name: 'search', description: `Discover codemode tool declarations. Access: ${full ? 'full' : 'read-only'}. Empty query lists tools.`, inputSchema: schema({ query: string }), annotations: { readOnlyHint: true, openWorldHint: false } },
  { name: 'execute', description: 'Execute JavaScript using discovered tools; await every call and return a small result or use text(). No Node, filesystem, or network APIs inside the VM. 60-second deadline, 64 MiB heap, 100 tool calls, bounded output. Side effects before errors are not rolled back. store/load persist only for this MCP process.', inputSchema: schema({ code: string }, ['code']), annotations: { readOnlyHint: !full, destructiveHint: full, openWorldHint: full } },
] }));
server.setRequestHandler(CallToolRequestSchema, async (request, extra) => {
  try {
    if (request.params.name === 'search') {
      const query = request.params.arguments?.query ?? '';
      if (typeof query !== 'string') throw new Error('query must be a string');
      const words = query.toLowerCase().split(/\s+/).filter(Boolean);
      const matches = tools.filter(t => words.every(w => `${t.name} ${t.description}`.toLowerCase().includes(w)));
      const text = matches.slice(0, 25).map(t => renderToolSample(t)).join('\n\n');
      return { content: [{ type: 'text', text: `${text || 'No matching tools.'}\n${matches.length} matches; showing at most 25.\n${connectionErrors.join('\n')}`.slice(0, MAX_OUTPUT) }] };
    }
    if (request.params.name !== 'execute') throw new Error('Unknown tool');
    const code = required(request.params.arguments?.code, 'code');
    if (Buffer.byteLength(code) > 65_536) throw new Error('Script exceeds 64 KiB');
    if (executing) throw new Error('Another script is running; retry after it finishes');
    executing = true; calls = 0;
    try {
      const result = await sandbox.execute(code, { signal: extra.signal, store });
      if (result.ok) {
        for (const key of result.storeWrites.delete) delete store[key];
        Object.assign(store, result.storeWrites.set);
      }
      let budget = MAX_OUTPUT;
      let imageBudget = 2 * MAX_DATA;
      const content = [];
      for (const item of result.output) {
        if (item.type === 'image') {
          if (item.data.length <= imageBudget) { content.push(item); imageBudget -= item.data.length; }
          else content.push({ type: 'text', text: '[Image omitted: exceeds 2 MiB]' });
        } else if (budget > 0) {
          content.push({ type: 'text', text: item.text.slice(0, budget) }); budget -= item.text.length;
        }
      }
      const value = result.ok ? (result.value === undefined ? '' : JSON.stringify(result.value)) : result.error.message;
      if (value) content.push({ type: 'text', text: value.slice(0, Math.max(0, budget)) });
      content.push({ type: 'text', text: JSON.stringify({ ok: result.ok, calls: result.calls, truncated: budget < 0 || value.length > Math.max(0, budget) }) });
      return { content, isError: !result.ok };
    } finally { executing = false; }
  } catch (error) { return { content: [{ type: 'text', text: error.message }], isError: true }; }
});
let closing = false;
async function close() {
  if (closing) return;
  closing = true;
  await sandbox.close();
  await Promise.allSettled(clients.map(c => c.close()));
  await server.close();
}
process.on('SIGTERM', () => { void close().finally(() => process.exit(0)); });
process.on('SIGINT', () => { void close().finally(() => process.exit(0)); });
process.stdin.on('end', () => { void close(); });
await server.connect(new StdioServerTransport());
