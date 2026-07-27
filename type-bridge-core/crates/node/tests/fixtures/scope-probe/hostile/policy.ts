const compiler = new QueryCompiler();
native["execute" + "_query"](compiler);
const policy = { "compileQuery": compiler };
