/** Quote an identifier only when needed. */
export function quoteIdent(name: string): string {
  return /^[a-z_][a-z0-9_]*$/.test(name) ? name : `"${name.replace(/"/g, '""')}"`;
}

export const sqlString = (s: string) => `'${s.replace(/'/g, "''")}'`;
