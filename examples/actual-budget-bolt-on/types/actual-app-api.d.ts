// Ambient fallback so this example type-checks WITHOUT pulling Actual's (native-dep-heavy)
// package into the monorepo. We dynamically import `@actual-app/api` at runtime and cast it to
// our own `ActualApi` interface, so an untyped module declaration is all tsc needs here. When
// you `npm install @actual-app/api` to run the demo for real, the package's own types take
// precedence over this declaration.
declare module "@actual-app/api";
