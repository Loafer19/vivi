// One-click MCP install links for the vivi-mcp stdio server.
// Cursor: cursor://anysphere.cursor-deeplink/mcp/install?name=…&config=<base64 JSON>
//   (official docs: the config is the server object, not the mcpServers wrapper).
// VS Code: vscode:mcp/install?<url-encoded JSON>, with name inside the object.

const SERVER_NAME = 'vivi';
const CURSOR_CONFIG = { command: 'vivi-mcp', args: [] };
const VSCODE_CONFIG = { name: 'vivi', type: 'stdio', command: 'vivi-mcp' };

/**
 * Cursor install deeplink for the vivi-mcp stdio server.
 * @returns {string}
 */
function cursorLink() {
  const config = btoa(JSON.stringify(CURSOR_CONFIG));
  return (
    'cursor://anysphere.cursor-deeplink/mcp/install?name=' +
    encodeURIComponent(SERVER_NAME) +
    '&config=' +
    encodeURIComponent(config)
  );
}

/**
 * VS Code / Copilot install deeplink for the vivi-mcp stdio server.
 * @returns {string}
 */
function vscodeLink() {
  return 'vscode:mcp/install?' + encodeURIComponent(JSON.stringify(VSCODE_CONFIG));
}

function init() {
  for (const link of document.querySelectorAll('[data-mcp]')) {
    if (link.dataset.mcp === 'cursor') {
      link.href = cursorLink();
    }
    if (link.dataset.mcp === 'vscode') {
      link.href = vscodeLink();
    }
  }
}

init();
