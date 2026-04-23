{ lib, python3Packages, fetchFromGitHub }:

python3Packages.buildPythonApplication rec {
  pname = "spec-kit";
  version = "0.8.0";

  src = fetchFromGitHub {
    owner = "github";
    repo = "spec-kit";
    rev = "v${version}";
    sha256 = "sha256-BJfIlLWRY1EUtitNeqwp1THNVwnQpvSpd2IgNLMAeuo=";
  };

  format = "pyproject";

  nativeBuildInputs = with python3Packages; [
    hatchling
  ];

  propagatedBuildInputs = with python3Packages; [
    typer
    click
    rich
    platformdirs
    readchar
    pyyaml
    packaging
    pathspec
    json5
  ];

  pythonImportsCheck = [ "specify_cli" ];

  meta = with lib; {
    description = "A toolkit to help developers get started with Spec-Driven Development";
    homepage = "https://github.com/github/spec-kit";
    license = licenses.mit;
    maintainers = [ ];
    mainProgram = "specify";
  };
}