import { readFile } from "fs/promises";

interface User {
  id: number;
  name: string;
}

export class Store<T extends User> {
  private items: Map<number, T> = new Map();

  add(item: T): void {
    this.items.set(item.id, item);
  }

  find(id: number): T | undefined {
    return this.items.get(id);
  }
}

async function main(): Promise<void> {
  const text = await readFile("users.json", "utf8");
  const users: User[] = JSON.parse(text);
  const names = users
    .filter((u) => u.id > 0)
    .map((u) => u.name);
  switch (names.length) {
    case 0:
      console.log("none");
      break;
    default:
      console.log(`found ${names.length}`);
  }
}
