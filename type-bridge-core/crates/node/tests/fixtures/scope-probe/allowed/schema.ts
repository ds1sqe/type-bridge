/** TypeQL schema importer/generator documentation may name QueryCompiler and execute_query. */
export const importedTypeql = "define entity person;";
export const boundaryDescription =
  "QueryCompiler execute_query typedb-driver are not owned here";

export function generateDefineBlockJson(): string {
  return importedTypeql;
}
