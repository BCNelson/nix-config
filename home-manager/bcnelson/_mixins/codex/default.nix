_: {
  programs.codex = {
    enable = true;
    enableMcpIntegration = true;
    settings = {
      experimental_use_rmcp_client = true;
    };
  };
}
