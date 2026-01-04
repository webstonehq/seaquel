import Database from 'better-sqlite3';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const dbPath = join(__dirname, 'test.db');

// Remove existing database if it exists
import { unlinkSync, existsSync } from 'fs';
if (existsSync(dbPath)) {
    unlinkSync(dbPath);
}

const db = new Database(dbPath);

// Enable foreign keys
db.pragma('foreign_keys = ON');

// Create tables
db.exec(`
-- Departments table
CREATE TABLE departments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    budget REAL CHECK(budget >= 0),
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);

-- Employees table (references departments)
CREATE TABLE employees (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    email TEXT NOT NULL UNIQUE,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    department_id INTEGER,
    salary REAL CHECK(salary > 0),
    hire_date TEXT NOT NULL,
    is_active INTEGER DEFAULT 1 CHECK(is_active IN (0, 1)),
    FOREIGN KEY (department_id) REFERENCES departments(id) ON DELETE SET NULL
);

-- Projects table
CREATE TABLE projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    department_id INTEGER NOT NULL,
    start_date TEXT NOT NULL,
    end_date TEXT,
    status TEXT DEFAULT 'active' CHECK(status IN ('active', 'completed', 'cancelled')),
    FOREIGN KEY (department_id) REFERENCES departments(id) ON DELETE CASCADE
);

-- Employee-Project assignments (many-to-many junction table)
CREATE TABLE project_assignments (
    employee_id INTEGER NOT NULL,
    project_id INTEGER NOT NULL,
    role TEXT NOT NULL,
    assigned_at TEXT DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (employee_id, project_id),
    FOREIGN KEY (employee_id) REFERENCES employees(id) ON DELETE CASCADE,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

-- Indexes
CREATE INDEX idx_employees_department ON employees(department_id);
CREATE INDEX idx_employees_email ON employees(email);
CREATE INDEX idx_employees_name ON employees(last_name, first_name);
CREATE INDEX idx_projects_department ON projects(department_id);
CREATE INDEX idx_projects_status ON projects(status);
`);

// Generate 500 additional tables for testing large schema
console.log('Creating 500 tables...');
for (let i = 1; i <= 500; i++) {
    const tableName = `table_${String(i).padStart(3, '0')}`;
    db.exec(`
        CREATE TABLE ${tableName} (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            value REAL,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX idx_${tableName}_name ON ${tableName}(name);
    `);
}

// Insert sample data

// Departments
const insertDept = db.prepare('INSERT INTO departments (name, budget) VALUES (?, ?)');
insertDept.run('Engineering', 500000);
insertDept.run('Marketing', 200000);
insertDept.run('Sales', 300000);
insertDept.run('Human Resources', 150000);

// Employees - generate 100,000 employees
const firstNames = ['Alice', 'Bob', 'Carol', 'David', 'Emma', 'Frank', 'Grace', 'Henry', 'Ivy', 'Jack', 'Kate', 'Liam', 'Mia', 'Noah', 'Olivia', 'Paul', 'Quinn', 'Rose', 'Sam', 'Tina', 'Uma', 'Victor', 'Wendy', 'Xavier', 'Yara', 'Zach'];
const lastNames = ['Johnson', 'Smith', 'Williams', 'Brown', 'Davis', 'Miller', 'Wilson', 'Moore', 'Taylor', 'Anderson', 'Thomas', 'Jackson', 'White', 'Harris', 'Martin', 'Thompson', 'Garcia', 'Martinez', 'Robinson', 'Clark', 'Rodriguez', 'Lewis', 'Lee', 'Walker', 'Hall', 'Allen'];

const insertEmployee = db.prepare(`
    INSERT INTO employees (email, first_name, last_name, department_id, salary, hire_date, is_active)
    VALUES (?, ?, ?, ?, ?, ?, ?)
`);

const insertManyEmployees = db.transaction(() => {
    for (let i = 0; i < 100000; i++) {
        const firstName = firstNames[i % firstNames.length];
        const lastName = lastNames[Math.floor(i / firstNames.length) % lastNames.length];
        const email = `${firstName.toLowerCase()}.${lastName.toLowerCase()}.${i}@example.com`;
        const departmentId = (i % 4) + 1;
        const salary = 50000 + Math.floor(Math.random() * 100000);
        const year = 2018 + (i % 7);
        const month = String((i % 12) + 1).padStart(2, '0');
        const day = String((i % 28) + 1).padStart(2, '0');
        const hireDate = `${year}-${month}-${day}`;
        const isActive = i % 20 === 0 ? 0 : 1; // 5% inactive

        insertEmployee.run(email, firstName, lastName, departmentId, salary, hireDate, isActive);
    }
});

console.log('Inserting 100,000 employees...');
insertManyEmployees();

// Projects
const insertProject = db.prepare(`
    INSERT INTO projects (name, description, department_id, start_date, end_date, status)
    VALUES (?, ?, ?, ?, ?, ?)
`);
insertProject.run('Website Redesign', 'Complete overhaul of company website', 1, '2024-01-01', '2024-06-30', 'active');
insertProject.run('Mobile App v2', 'Second version of mobile application', 1, '2023-06-01', '2024-03-15', 'completed');
insertProject.run('Q1 Marketing Campaign', 'Social media and email marketing push', 2, '2024-01-15', '2024-03-31', 'active');
insertProject.run('Sales Automation', 'Implement CRM automation workflows', 3, '2024-02-01', null, 'active');
insertProject.run('Employee Portal', 'Internal HR self-service portal', 4, '2023-09-01', '2024-01-31', 'completed');

// Project assignments
const insertAssignment = db.prepare(`
    INSERT INTO project_assignments (employee_id, project_id, role)
    VALUES (?, ?, ?)
`);
insertAssignment.run(1, 1, 'Tech Lead');
insertAssignment.run(2, 1, 'Developer');
insertAssignment.run(3, 1, 'Developer');
insertAssignment.run(1, 2, 'Tech Lead');
insertAssignment.run(2, 2, 'Developer');
insertAssignment.run(4, 3, 'Project Manager');
insertAssignment.run(5, 3, 'Marketing Specialist');
insertAssignment.run(6, 4, 'Project Manager');
insertAssignment.run(7, 4, 'Sales Analyst');
insertAssignment.run(9, 5, 'Project Manager');
insertAssignment.run(10, 5, 'HR Specialist');
insertAssignment.run(3, 5, 'Developer');

// Insert sample data into the 500 generated tables
console.log('Inserting sample data into 500 tables...');
const insertManyTables = db.transaction(() => {
    for (let i = 1; i <= 500; i++) {
        const tableName = `table_${String(i).padStart(3, '0')}`;
        const insertRow = db.prepare(`INSERT INTO ${tableName} (name, value) VALUES (?, ?)`);
        for (let j = 1; j <= 10; j++) {
            insertRow.run(`item_${j}`, Math.random() * 1000);
        }
    }
});
insertManyTables();

db.close();

console.log(`Database created successfully at: ${dbPath}`);
console.log('Tables: 504 total (4 core + 500 generated)');
console.log('Sample data: 4 departments, 100,000 employees, 5 projects, 12 assignments, 5,000 rows in generated tables');
