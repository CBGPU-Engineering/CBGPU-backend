import 'node:http';
declare module 'node:http' {interface IncomingMessage {auditActor?:Record<string,any>}}
