import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { ListToolsRequestSchema, CallToolRequestSchema } from '@modelcontextprotocol/sdk/types.js';
const server = new Server({ name: 'fixture', version: '1' }, { capabilities: { tools: {} } });
server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: [{ name: 'double', description: 'Double a number', inputSchema: { type: 'object', properties: { value: { type: 'number' } }, required: ['value'] } }] }));
server.setRequestHandler(CallToolRequestSchema, async r => ({ content: [{ type: 'text', text: String(r.params.arguments.value * 2) }], structuredContent: { value: r.params.arguments.value * 2 } }));
await server.connect(new StdioServerTransport());
