import type { Storage, Transaction } from "./engine.ts";
/** The small workerd KV surface used by cwpdb; no application SQL escapes. */
export interface KvTransaction {
  get<T>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
}
export interface KvStorage extends KvTransaction {
  transaction<T>(work: (transaction: KvTransaction) => Promise<T>): Promise<T>;
}
interface Manifest {
  cwpdb_chunks_v1: number;
}
const MAX_CHUNK = 60 * 1024;
class ChunkTransaction implements Transaction {
  constructor(private readonly kv: KvTransaction) {}
  async get<T>(key: string): Promise<T | undefined> {
    const manifest = await this.kv.get<Manifest>(`manifest:${key}`);
    if (!manifest) return undefined;
    if (
      !Number.isSafeInteger(manifest.cwpdb_chunks_v1) ||
      manifest.cwpdb_chunks_v1 < 1 || manifest.cwpdb_chunks_v1 > 410
    ) throw new Error("invalid cwpdb chunk manifest");
    const chunks: Uint8Array[] = [];
    for (let i = 0; i < manifest.cwpdb_chunks_v1; i++) {
      const chunk = await this.kv.get<Uint8Array>(`chunk:${i}:${key}`);
      if (!chunk) throw new Error("incomplete cwpdb record");
      chunks.push(chunk);
    }
    const bytes = new Uint8Array(
      chunks.reduce((sum, chunk) => sum + chunk.length, 0),
    );
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.length;
    }
    return JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(bytes),
    ) as T;
  }
  async put<T>(key: string, value: T): Promise<void> {
    const bytes = new TextEncoder().encode(JSON.stringify(value));
    if (bytes.length > 24 * 1024 * 1024) {
      throw new Error("cwpdb internal record exceeds24MiB");
    }
    const count = Math.max(1, Math.ceil(bytes.length / MAX_CHUNK));
    const old = await this.kv.get<Manifest>(`manifest:${key}`);
    for (let i = 0; i < count; i++) {
      await this.kv.put(
        `chunk:${i}:${key}`,
        bytes.slice(i * MAX_CHUNK, (i + 1) * MAX_CHUNK),
      );
    }
    for (let i = count; i < (old?.cwpdb_chunks_v1 ?? 0); i++) {
      await this.kv.delete(`chunk:${i}:${key}`);
    }
    await this.kv.put<Manifest>(`manifest:${key}`, { cwpdb_chunks_v1: count });
  }
  async delete(key: string): Promise<void> {
    const old = await this.kv.get<Manifest>(`manifest:${key}`);
    for (let i = 0; i < (old?.cwpdb_chunks_v1 ?? 0); i++) {
      await this.kv.delete(`chunk:${i}:${key}`);
    }
    await this.kv.delete(`manifest:${key}`);
  }
}
export class DurableKv extends ChunkTransaction implements Storage {
  constructor(private readonly storage: KvStorage) {
    super(storage);
  }
  transaction<T>(work: (transaction: Transaction) => Promise<T>): Promise<T> {
    return this.storage.transaction((tx) => work(new ChunkTransaction(tx)));
  }
}
