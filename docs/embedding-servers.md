# Embedding Servers

Lore turns each pattern section into a vector with an embedding model. By default, that model runs
in Ollama. An `[embedding]` table points lore at an OpenAI-compatible embedding server instead. Such
a server answers embedding requests in the format of OpenAI's API, as `llama-server`, LM Studio and
some hosted services do.

## Start the Server

Start the server with an embedding model loaded. For `llama-server`, pass `--embeddings`:

```sh
llama-server --embeddings -m nomic-embed-text-v1.5.Q8_0.gguf --port 8080 -ub 1024 -b 1024
```

A default `llama-server` takes at most 512 tokens per input. Set `-ub` and `-b` to at least
`[chunking] max_tokens`, which is 1024 by default, so long sections fit.

LM Studio drops any input beyond the model's loaded context without an error, so give the model a
context of at least twice `[chunking] max_tokens`.

## Set Up Lore

Run `lore init` with the server's base URL and a model name:

```sh
lore init --repo /path/to/your/patterns --provider openai-compatible \
    --url http://127.0.0.1:8080/v1 --model nomic-embed-text-v1.5
```

If the server needs a key, add `--api-key-environment-variable-name`, as
[Add an API Key](#add-an-api-key) shows.

For `llama-server` and LM Studio, the URL ends in `/v1`.

The model name is what lore sends with each request. `llama-server` answers with the model it
loaded, whatever the name says, so pick a name that matches that model. LM Studio embeds with the
model that the name picks. It answers an unknown name with the loaded model and no error, so use a
name that `GET {url}/models` lists.

Lore writes `lore.toml` with an `[embedding]` table in place of `[ollama]`. Then it checks that the
file loads and that the server returns a vector. After that, it builds the index. It installs no
model, because the server already holds one.

If a check fails, `lore init` exits 1 and keeps the file.

The [Configuration Reference](configuration.md#embedding-section) lists every field of the table.

## Add an API Key

If the server needs a key, keep the key in an environment variable. Then pass the variable's name to
`lore init` with `--api-key-environment-variable-name`, or name it in `[embedding]`:

```toml
api_key_environment_variable_name = "EMBEDDING_API_KEY"
```

The plugin's hooks and its Model Context Protocol (MCP) server read the key from your agent's
environment, so set the variable there too.

## Check the Server

The `Server:` line in `lore status` comes from `GET {url}/models`, so it shows that the server
answers, not that its model embeds.

## Long Sections

If `llama-server` still rejects a section as too long, lore stores the section without a vector. The
ingest prints the error and its remedy once:

```text
✗ The embedding server at http://127.0.0.1:8080/v1 rejected a section as too long. Start 'llama-server' with '-ub' and '-b' of at least 1024, or lower [chunking] max_tokens.
```

Restart the server with larger `-ub` and `-b`, then run `lore ingest`. It adds the missing vectors
without a rebuild. The [`max_tokens` row](configuration.md#chunking-section) says how much of each
section lore sends. The text index always keeps the whole section.

## Switch From Ollama

`lore init` never rewrites an existing `lore.toml`, so edit it yourself.
[File Paths](configuration.md#file-paths) shows where it is. Replace its `[ollama]` table with an
`[embedding]` table:

```toml
[embedding]
provider = "openai-compatible"
url = "http://127.0.0.1:8080/v1"
model = "nomic-embed-text-v1.5"
```

An index records the provider and model that built it. `lore init` rebuilds the index with the
server's vectors. If you skip `lore init`, run `lore ingest --force` once instead. Until then,
`lore ingest` exits 1 and searches use text only.

## Trace Records

SessionStart trace records name the server type in `config.embedder_provider`, as `ollama` or
`openai-compatible`. They keep the server's URL and model in `config.ollama_host` and
`config.ollama_model`, whichever server lore uses. The URL leaves out any user name and password.

## Security

Read [SECURITY.md](../SECURITY.md#embedding-servers) before you use a server on another machine or
set a proxy. It says what lore sends to the server, and where the API key can travel.
