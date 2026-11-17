import { App } from "aws-cdk-lib";
import { CloudReadinessStack } from "./stack";
const app = new App();
new CloudReadinessStack(app, "CBGPUCloudReadiness");
app.synth();
