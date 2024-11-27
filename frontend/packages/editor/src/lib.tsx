import {
  type AquascopeBackend,
  initializeAquascopeInstance
} from "@aquascope/system";
import { rust } from "@codemirror/lang-rust";
import { indentUnit } from "@codemirror/language";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { EditorView, type ViewUpdate } from "@codemirror/view";
import React, {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState
} from "react";
import ReactDOM from "react-dom/client";

import { boundariesField } from "./editor-utils/boundaries.js";
import {
  type InterpreterConfig,
  markerField,
  renderInterpreter
} from "./editor-utils/interpreter.js";
import {
  type IconField,
  hiddenLines,
  hideLine,
  loanFactsField
} from "./editor-utils/misc.js";
import {
  type PermissionsConfig,
  type PermissionsDecorations,
  makePermissionsDecorations,
  renderPermissions
} from "./editor-utils/permissions.js";
import { stepField } from "./editor-utils/stepper.js";
import "./styles.scss";
import {
  type AnalysisFacts,
  type AnalysisOutput,
  type AquascopeAnnotations,
  type BackendError,
  type CharRange,
  type Command,
  type CommandResult,
  type InterpAnnotations,
  type MTrace,
  isInterp,
  isPerm
} from "@aquascope/types";

export * as types from "@aquascope/types";

export const defaultCodeExample: string = `
fn main() {
  let mut v = vec![1, 2, 3];
  let n = &v[0];
  v.push(0);
  let x = *n;
}
`.trim();

const readOnly = new Compartment();
const mainKeybinding = new Compartment();

type ButtonName = "copy" | "eye";

const CopyButton = ({ view }: { view: EditorView }) => (
  <button
    type="button"
    className="cm-button"
    onClick={() => {
      let contents = view.state.doc.toJSON().join("\n");
      navigator.clipboard.writeText(contents);
    }}
  >
    <i className="fa fa-copy" />
  </button>
);

const HideButton = ({ container }: { container: HTMLDivElement }) => {
  const [hidden, setHidden] = useState(true);
  useEffect(() => {
    if (!hidden) container.classList.add("show-hidden");
    else container.classList.remove("show-hidden");
  }, [hidden]);
  return (
    // biome-ignore lint/a11y/useButtonType: TODO
    <button className="cm-button" onClick={() => setHidden(!hidden)}>
      <i className={`fa ${hidden ? "fa-eye" : "fa-eye-slash"}`} />
    </button>
  );
};

const resetMarkedRangesOnEdit = EditorView.updateListener.of(
  (upd: ViewUpdate) => {
    if (upd.docChanged) {
      upd.view.dispatch({
        effects: [
          boundariesField.setEffect.of([]),
          stepField.setEffect.of([]),
          markerField.setEffect.of([])
        ]
      });
    }
  }
);

interface CommonConfig {
  shouldFail?: boolean;
  stepper?: any;
  boundaries?: any;
}

type CommandConfig<C extends Command> = 
  C extends "interpreter" 
  ? InterpreterConfig 
  : C extends "permissions" 
  ? PermissionsConfig 
  : never;

export class Editor {
  private view: EditorView;
  private interpreterContainer: HTMLDivElement;
  private editorContainer: HTMLDivElement;
  private permissionsDecos?: PermissionsDecorations;
  private metaContainer: ReactDOM.Root;
  private buttons: Set<ButtonName>;
  private shouldFail = false;

  constructor(
    dom: HTMLDivElement,
    readonly setup: Extension,
    readonly backend: AquascopeBackend,
    readonly reportStdErr: (err: BackendError) => void = err => {
      console.error("An error occurred: ");
      console.error(err);
    },
    code: string = defaultCodeExample,
    readonly noInteract: boolean = false,
    readonly shouldFailHtml: string = "This code does not compile!",
    readonly buttonList: ButtonName[] = []
  ) {
    this.buttons = new Set(buttonList);

    const initialState = EditorState.create({
      doc: code,
      extensions: [
        mainKeybinding.of(setup),
        readOnly.of(EditorState.readOnly.of(noInteract)),
        EditorView.editable.of(!noInteract),
        resetMarkedRangesOnEdit,
        setup,
        rust(),
        indentUnit.of("  "),
        hiddenLines,
        loanFactsField,
        boundariesField.field,
        stepField.field,
        markerField.field
      ]
    });

    console.debug(`Creating Rust editor with code: ${code}`);

    this.editorContainer = document.createElement("div");
    this.view = new EditorView({
      state: initialState,
      parent: this.editorContainer
    });

    let buttonContainer = document.createElement("div");
    this.metaContainer = ReactDOM.createRoot(buttonContainer);
    this.renderMeta();

    this.editorContainer.appendChild(buttonContainer);

    this.interpreterContainer = document.createElement("div");

    dom.appendChild(this.editorContainer);
    dom.appendChild(this.interpreterContainer);
  }

  renderMeta() {
    this.metaContainer.render(
      <div className="meta-container">
        <div className="top-right">
          {Array.from(this.buttons).map((button, i) =>
            button === "copy" ? (
              <CopyButton key={i} view={this.view} />
            ) : button === "eye" ? (
              <HideButton key={i} container={this.editorContainer} />
            ) : null
          )}
        </div>
        {this.shouldFail ? (
          <div
            // biome-ignore lint/security/noDangerouslySetInnerHtml: not user-configurable
            dangerouslySetInnerHTML={{ __html: this.shouldFailHtml }}
          />
        ) : null}
      </div>
    );
  }

  public getCurrentCode(): string {
    return this.view.state.doc.toString();
  }

  public reconfigure(extensions: Extension[]): void {
    this.view.dispatch({
      effects: [mainKeybinding.reconfigure([...extensions, this.setup])]
    });
  }

  public removeIconField<B, T, F extends IconField<B, T>>(f: F) {
    this.view.dispatch({
      effects: [f.effectType.of([])]
    });
  }

  public addPermissionsField<B, T, F extends IconField<B, T>>(
    f: F,
    methodCallPoints: Array<B>,
    facts: AnalysisFacts
  ) {
    let newEffects = methodCallPoints.map(v => f.fromOutput(v, facts));
    this.view.dispatch({
      effects: [f.effectType.of(newEffects)]
    });
  }

  public async renderPermissions(cfg?: PermissionsConfig) {
    // TODO: the permissions Decos are no longer removed on update
    // so we have to recompute every time.
    await this.renderOperation("permissions", {
      config: cfg
    });

    renderPermissions(this.view, this.permissionsDecos, cfg);
  }

  public destroy() {
    this.view.destroy();
  }

  renderInterpreter(
    trace: MTrace<CharRange>,
    config?: InterpreterConfig,
    annotations?: InterpAnnotations
  ) {
    if (config?.hideCode) {
      this.view.destroy();
      this.metaContainer.unmount();
    }

    renderInterpreter(
      this.view,
      this.interpreterContainer,
      trace,
      config,
      annotations
    );
  }

  async renderOperation<C extends Command>(
    operation: C,
    {
      response,
      config,
      annotations
    }: {
      response?: CommandResult<typeof operation>;
      config?: CommonConfig & CommandConfig<C> & { shouldFail?: boolean };
      annotations?: AquascopeAnnotations;
    } = {}
  ) {
    console.debug(`Rendering operation: ${operation}`);
    const inEditor = this.getCurrentCode();

    if (config?.shouldFail) {
      this.shouldFail = true;
    }

    if (!response) {
      response = await this.backend.call(operation, inEditor, {
        shouldFail: this.shouldFail
      });

      if (!response) {
        this.reportStdErr({
          type: "AnalysisError",
          msg: `render operation ${operation} failed`
        });
        return;
      }
    }

    if (annotations?.hidden_lines && annotations.hidden_lines.length > 0) {
      this.view.dispatch({
        effects: annotations.hidden_lines.map(line => hideLine.of({ line }))
      });
      this.buttons.add("eye");
    }

    this.renderMeta();

    if (isInterp(operation)) {
      // TODO: I don't like that we need this to narrow the type of response
      response = response as CommandResult<typeof operation>;
      if ("Ok" in response) {
        this.renderInterpreter(response.Ok, config as any, annotations?.interp);
      } else {
        this.reportStdErr(response.Err);
      }
    } else if (isPerm(operation)) {
      // TODO: I don't like that we need this to narrow the type of response
      const resp: CommandResult<typeof operation> = response;
      // The permissions analysis results are sent as an array of
      // body analyses. Each body could have analyzed successfully,
      // or had a
      // 1. analysis error
      // 2. build error
      // A build error signifies that something went wrong *before*
      // our analysis was run. This should be reported to the user,
      // currently, information is available on stderr but nothing
      // more specific (or visual) is given TODO.
      // For an analysis error, this is something that went wrong
      // internally, usually means a feature was used that we don't support
      // or something actually went terribly wrong. These should be logged
      // somewhere, but the user should also be prompted to open a GitHub issue.
      console.warn("Permissions response: ", resp);
      const results: AnalysisOutput[] = [];
      for (const res of resp) {
        if ("Ok" in res) {
          results.push(res.Ok);
        } else {
          this.reportStdErr(res.Err);
        }
      }

      this.permissionsDecos = makePermissionsDecorations(
        this.view,
        results,
        annotations
      );
      renderPermissions(this.view, this.permissionsDecos, config);
    }
  }
}

export interface EditorComponentProps {
  code: string;
  setup?: Extension;
  permissions?: boolean;
  interpreter?: boolean;
  shouldFail?: boolean;
  showSteps?: boolean;
  showBoundaries?: boolean;
}

export const AquascopeContext = createContext<AquascopeBackend | null>(null);

export const AquascopeBackendProvider = (props: React.PropsWithChildren) => {
  const [backend, setBackend] = useState<AquascopeBackend | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    console.log("Starting initialization...");

    const init = async () => {
      console.info("Initializing...");
      setIsLoading(true);
      try {
        console.log("Calling initializeAquascopeInstance...");
        const b = await Promise.race([
          initializeAquascopeInstance(),
          new Promise((_, reject) =>
            setTimeout(() => reject(new Error("Initialization timeout")), 5000)
          )
        ]);

        console.log("Backend initialized successfully");
        setBackend(b as AquascopeBackend);
      } catch (e) {
        const errorMessage = e instanceof Error ? e.message : String(e);
        console.error("Backend initialization failed:", errorMessage);
        setError(errorMessage);
      } finally {
        console.log("Setting isLoading to false");
        setIsLoading(false);
      }
    };

    if (isLoading) {
      init();
    }
  }, []);

  if (isLoading) {
    return <div>Loading Aquascope backend...</div>;
  }

  if (error || !backend) {
    return <div>Failed to initialize Aquascope backend: {error}</div>;
  }

  return (
    <AquascopeContext.Provider value={backend}>
      {props.children}
    </AquascopeContext.Provider>
  );
};

export const EditorComponent = (props: EditorComponentProps) => {
  const ref = useRef<HTMLDivElement>(null);
  const backend = useContext(AquascopeContext);
  const [editor, setEditor] = useState<Editor | null>(null);

  const [passesBorrowChecker, setPassesBorrowChecker] = useState(false);
  const [showBoundaries, setShowBoundaries] = useState(props.showBoundaries ?? true);
  const [showSteps, setShowSteps] = useState(props.showSteps ?? true);


  const renderPermissions = () => {
    editor?.renderPermissions({
      stepper: showSteps,
      boundaries: showBoundaries,
    });
  };

  const renderInterpreter = () => {
    editor?.renderOperation("interpreter", {
      shouldFail: !passesBorrowChecker,
      horizontal: true
    } as any);
  };

  useEffect(() => {
    if (!backend) {
      console.warn("Aquascope backend not initialized, deferring editor setup");
      return;
    }

    console.debug("Backend initialized, setting up editor");

    const editor = new Editor(
      ref.current!,
      props.setup ?? [],
      backend!,
      (err: BackendError) => {
        console.error(`An error occurred: ${err}`);
      },
      props.code
    );

    setEditor(editor);
    if (props.permissions) renderPermissions();
    if (props.interpreter) renderInterpreter();

    return () => editor.destroy();
  }, [backend]);

  const buttons = (
    <div className="buttons">
      <div>
        <div className="dropdown">
          <button className="cm-button">            
            <i className="fa fa-caret-down"></i>
          </button>
          <div className="dropdown-content">
            <label className="dropdown-container">Must pass borrow checker?
              <input type="checkbox" id="passesBorrowChecker" checked={passesBorrowChecker} onChange={() => setPassesBorrowChecker(!passesBorrowChecker)}/>
              <span className="checkmark" />
            </label>           
          </div>
        </div>
        <button className="cm-button" onClick={renderInterpreter}>Interpret<span className="loader" /></button>
      </div>
      <div>
        <div className="dropdown">
          <button className="cm-button">
            <i className="fa fa-caret-down"></i>
          </button>
          <div className="dropdown-content">
            <label className="dropdown-container">Boundaries
              <input type="checkbox" id="showBoundaries" checked={showBoundaries} onChange={() => setShowBoundaries(!showBoundaries)}/>
              <span className="checkmark" />
            </label>
            <label className="dropdown-container">Steps
              <input type="checkbox" id="showSteps" checked={showSteps} onChange={() => setShowSteps(!showSteps)}/>
              <span className="checkmark" />
            </label>
          </div>
        </div>
        <button className="cm-button" onClick={renderPermissions}>Permissions<span className="loader" /></button>
      </div>
    </div>
  );

  return !backend ? (
    <div>Waiting for Aquascope backend...</div>
  ) : (
    <div className="aquascope-container">
      <div className="instance" ref={ref} />
      {buttons}
    </div>
  );
};
