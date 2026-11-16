import type {DatabaseSync} from 'node:sqlite';
/** Legacy SQL has heterogeneous result projections; runtime validation remains at each API boundary. */
export type SqlRow = Record<string, any>;
export type LegacyDatabase = Omit<DatabaseSync,'prepare'> & {prepare(sql: string): {
 get(...parameters: unknown[]): SqlRow | undefined;
 all(...parameters: unknown[]): SqlRow[];
 run(...parameters: unknown[]): {changes:number;lastInsertRowid:number|bigint};
 columns(): Array<{name:string;column:string|null;database:string|null;table:string|null;type:string|null}>;
 setAllowBareNamedParameters(value:boolean):void;
 setReadBigInts(value:boolean):void;
}};
