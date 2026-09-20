{ lib, python3Packages, fetchFromGitHub }:

python3Packages.buildPythonApplication rec {
  pname = "kobodl";
  version = "0.14.0";

  src = fetchFromGitHub {
    owner = "subdavis";
    repo = "kobo-book-downloader";
    rev = version;
    hash = "sha256-z1q5kqcyJFbmRzQQyAIjDk3lBholwcKsbrsss5eOumQ=";
  };

  format = "pyproject";

  nativeBuildInputs = with python3Packages; [
    poetry-core
    pythonRelaxDepsHook
  ];

  # Upstream pins hard (flask == 3.1.1, dataclasses-json < 0.6, tabulate < 0.9,
  # setuptools < 79); nixpkgs carries newer ones. `dataclasses` is the stdlib
  # backport for Python < 3.7 and has no nixpkgs attribute at all.
  pythonRelaxDeps = [
    "dataclasses-json"
    "flask"
    "setuptools"
    "tabulate"
  ];
  pythonRemoveDeps = [ "dataclasses" ];

  propagatedBuildInputs = with python3Packages; [
    beautifulsoup4
    click
    dataclasses-json
    flask
    pycryptodome
    requests
    setuptools
    tabulate
  ];

  pythonImportsCheck = [ "kobodl" ];

  meta = with lib; {
    description = "Kobo book downloader: fetches and removes DRM from your Kobo library";
    homepage = "https://github.com/subdavis/kobo-book-downloader";
    license = licenses.unlicense;
    maintainers = [ ];
    mainProgram = "kobodl";
  };
}
