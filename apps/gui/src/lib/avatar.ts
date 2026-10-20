import {
  Cat,
  Dog,
  Rabbit,
  Panda,
  Squirrel,
  Turtle,
  Bird,
  Fish,
  Snail,
  Origami,
} from '@lucide/svelte';

const animals = [
  Cat,
  Dog,
  Rabbit,
  Panda,
  Squirrel,
  Turtle,
  Bird,
  Fish,
  Snail,
  Origami,
];

export function agentAvatar(personId: string, handle: string) {
  // Hash identity bytes, so renaming a person or changing devices keeps their animal.
  let hash = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(`${personId}/${handle}`)) {
    hash = Math.imul(hash ^ byte, 0x01000193) >>> 0;
  }
  return animals[hash % animals.length];
}
